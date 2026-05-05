//! Global task registry for tracking long-running background operations.
//!
//! Each task is assigned a UUID and stores a complete event log. New subscribers
//! replay the full history and then receive live events. Multiple simultaneous
//! subscribers are supported.
//!
//! # Race-condition safety
//! Events are published by holding the registry `Mutex`. Subscribers create their
//! `watch::Receiver` under the same lock, so no events can be published between
//! "subscribe" and "start replay" without being visible through the receiver.

use colmap_openmvs_api::{TaskEvent, TaskId, TaskInfo, TaskKind, TaskState};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Maximum number of events buffered per task (oldest are dropped if exceeded).
const MAX_EVENTS: usize = 100_000;

pub struct TaskEntry {
    pub info: TaskInfo,
    /// Append-only event log; protected by a plain Mutex so it can be locked briefly.
    pub events: Arc<Mutex<Vec<TaskEvent>>>,
    /// Tracks current event count; subscribers call `.subscribe()` to get a `Receiver`.
    pub seq_sender: watch::Sender<usize>,
}

pub struct TaskRegistry {
    tasks: HashMap<TaskId, TaskEntry>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// Register a new task. Returns the task_id.
    pub fn create_task(&mut self, kind: TaskKind, context_key: String) -> TaskId {
        let id = uuid::Uuid::new_v4().to_string();
        let (seq_sender, _rx) = watch::channel(0usize);
        let created_at = chrono::Utc::now().to_rfc3339();
        let info = TaskInfo {
            id: id.clone(),
            kind,
            state: TaskState::Running,
            context_key,
            created_at: created_at.clone(),
            updated_at: created_at,
        };
        let entry = TaskEntry {
            info,
            events: Arc::new(Mutex::new(Vec::new())),
            seq_sender,
        };
        self.tasks.insert(id.clone(), entry);
        id
    }

    /// Publish an event to a task. Updates task state for terminal events.
    pub fn publish_event(&mut self, task_id: &str, event: TaskEvent) {
        if let Some(entry) = self.tasks.get_mut(task_id) {
            match &event {
                TaskEvent::Completed => {
                    entry.info.state = TaskState::Completed;
                }
                TaskEvent::Failed(msg) => {
                    entry.info.state = TaskState::Failed(msg.clone());
                }
                _ => {}
            }
            entry.info.updated_at = chrono::Utc::now().to_rfc3339();

            let new_seq = {
                let mut events = entry.events.lock().unwrap();
                events.push(event);
                // Trim if over limit (keeps newest)
                if events.len() > MAX_EVENTS {
                    let drop_count = events.len() - MAX_EVENTS;
                    events.drain(0..drop_count);
                }
                events.len()
            };
            // Notify all subscribers; ignore if no subscribers
            let _ = entry.seq_sender.send(new_seq);
        }
    }

    /// Subscribe to a task's events for replay+live streaming.
    /// Returns `None` if the task doesn't exist.
    pub fn subscribe(
        &self,
        task_id: &str,
    ) -> Option<(Arc<Mutex<Vec<TaskEvent>>>, watch::Receiver<usize>)> {
        self.tasks.get(task_id).map(|entry| {
            // IMPORTANT: subscribe to seq_sender BEFORE reading the events buffer
            // so we can't miss events published between the two operations.
            let rx = entry.seq_sender.subscribe();
            let events = entry.events.clone();
            (events, rx)
        })
    }

    /// List all tasks, newest first.
    pub fn list_tasks(&self) -> Vec<TaskInfo> {
        let mut tasks: Vec<_> = self.tasks.values().map(|e| e.info.clone()).collect();
        tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        tasks
    }

    /// Get info for a single task.
    pub fn get_task_info(&self, task_id: &str) -> Option<TaskInfo> {
        self.tasks.get(task_id).map(|e| e.info.clone())
    }

    /// Cancel a running task by publishing a Failed event.
    pub fn cancel_task(&mut self, task_id: &str) {
        self.publish_event(task_id, TaskEvent::Failed("Cancelled by user.".to_string()));
    }
}

/// The global task registry, shared across all requests.
pub static TASK_REGISTRY: Lazy<Arc<Mutex<TaskRegistry>>> =
    Lazy::new(|| Arc::new(Mutex::new(TaskRegistry::new())));

/// Convenience accessor for the global registry.
pub fn task_registry() -> Arc<Mutex<TaskRegistry>> {
    TASK_REGISTRY.clone()
}

/// Helper: publish an event to the global registry.
pub fn publish_event(task_id: &str, event: TaskEvent) {
    TASK_REGISTRY.lock().unwrap().publish_event(task_id, event);
}

/// Build an async stream that replays a task's event history and then streams
/// live events until a terminal event (`Completed` or `Failed`) is emitted.
///
/// This function is safe to call from multiple clients simultaneously.
pub fn create_event_stream(
    task_id: &str,
) -> Option<impl futures::Stream<Item = Result<TaskEvent, anyhow::Error>>> {
    let (events_arc, mut rx) = TASK_REGISTRY.lock().unwrap().subscribe(task_id)?;

    let (tx, stream_rx) = futures::channel::mpsc::unbounded::<Result<TaskEvent, anyhow::Error>>();

    tokio::spawn(async move {
        let mut cursor = 0usize;
        loop {
            // Mark the current seq as "seen" so changed() only triggers for NEW events.
            rx.borrow_and_update();

            // Drain all events since cursor.
            let batch: Vec<TaskEvent> = {
                let events = events_arc.lock().unwrap();
                events[cursor..].to_vec()
            };

            let mut is_terminal = false;
            for event in batch {
                cursor += 1;
                let terminal = matches!(event, TaskEvent::Completed | TaskEvent::Failed(_));
                if tx.unbounded_send(Ok(event)).is_err() {
                    return; // Client disconnected
                }
                if terminal {
                    is_terminal = true;
                    break;
                }
            }

            if is_terminal {
                break;
            }

            // Wait for the next event (or registry shutdown).
            if rx.changed().await.is_err() {
                break;
            }
        }
    });

    Some(stream_rx)
}
