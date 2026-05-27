//! Global task registry for tracking long-running background operations.
//!
//! Each task is assigned a UUID and stores a complete event log. New subscribers
//! replay the full history and then receive live events. Multiple simultaneous
//! subscribers are supported.
//!
//! # Locking design
//! The registry uses a `RwLock<HashMap<TaskId, Arc<TaskEntry>>>` so that:
//! - Only `create_task` (insert) takes a write lock — and only for the duration
//!   of the `HashMap::insert`.
//! - All other operations take a read lock ONLY long enough to clone the
//!   `Arc<TaskEntry>`, then release the lock immediately.
//! - Per-task mutations (`publish_event`, `cancel_task`, etc.) operate on the
//!   `Arc<TaskEntry>` with **no map lock held at all**.
//!
//! # Race-condition safety
//! The ordering guarantee is: `seq_sender.subscribe()` is called (in `subscribe()`)
//! BEFORE any read of `entry.events`. This means: if an event is published between
//! the time we clone the `Arc<TaskEntry>` and the time we first read the events
//! buffer, the `watch::Receiver` will reflect the new sequence number, and the
//! stream loop's `borrow_and_update()` will see it, ensuring the event is included
//! in the next batch read.

use colmap_openmvs_api::{TaskEvent, TaskId, TaskInfo, TaskKind, TaskState};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::watch;

/// Maximum number of events buffered per task (oldest are dropped if exceeded).
const MAX_EVENTS: usize = 100_000;

// Type alias for the subscriber tuple returned by `subscribe`.
type EventSubscriber = (Arc<TaskEntry>, watch::Receiver<usize>);

pub struct TaskEntry {
    /// Task metadata (state, timestamps, etc.); protected by a `Mutex` for
    /// brief, exclusive updates.
    pub info: Mutex<TaskInfo>,
    /// Append-only event log; protected by a `Mutex` so it can be locked briefly.
    pub events: Mutex<Vec<TaskEvent>>,
    /// Tracks current event count; subscribers call `.subscribe()` to get a
    /// `Receiver`. `watch::Sender` uses lock-free internal atomics.
    pub seq_sender: watch::Sender<usize>,
    /// Optional kill function for the running task; can be called to cancel execution.
    pub kill_fn: Mutex<Option<Box<dyn Fn() + Send>>>,
}

pub struct TaskRegistry {
    tasks: RwLock<HashMap<TaskId, Arc<TaskEntry>>>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskRegistry {
    /// Register a new task. Returns the `task_id`.
    ///
    /// The entire `Arc<TaskEntry>` is constructed before taking any lock; the
    /// write lock is held only for the `HashMap::insert` and released immediately.
    pub fn create_task(&self, kind: TaskKind, context_key: String) -> TaskId {
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
        let entry = Arc::new(TaskEntry {
            info: Mutex::new(info),
            events: Mutex::new(Vec::new()),
            seq_sender,
            kill_fn: Mutex::new(None),
        });

        // Write lock held ONLY for the insert — released immediately.
        self.tasks.write().unwrap().insert(id.clone(), entry);
        id
    }

    /// Publish an event to a task. Updates task state for terminal events.
    ///
    /// The map read lock is held only long enough to clone the `Arc<TaskEntry>`.
    /// All per-task mutations happen after the map lock is released.
    pub fn publish_event(&self, task_id: &str, event: TaskEvent) {
        // Step 1 — read lock: clone Arc, release immediately.
        let entry = {
            let tasks = self.tasks.read().unwrap();
            match tasks.get(task_id) {
                Some(e) => e.clone(),
                None => return,
            }
        };

        // Step 2 — update info (state + updated_at); no map lock held.
        {
            let mut info = entry.info.lock().unwrap();
            match &event {
                TaskEvent::Completed => {
                    info.state = TaskState::Completed;
                }
                TaskEvent::Failed(msg) => {
                    info.state = TaskState::Failed(msg.clone());
                }
                _ => {}
            }
            info.updated_at = chrono::Utc::now().to_rfc3339();
        }

        // Step 3 — append event, get new length; no map lock held.
        let new_seq = {
            let mut events = entry.events.lock().unwrap();
            events.push(event);
            // Trim if over limit (keeps newest).
            if events.len() > MAX_EVENTS {
                let drop_count = events.len() - MAX_EVENTS;
                events.drain(0..drop_count);
            }
            events.len()
        };

        // Step 4 — notify all subscribers; no lock needed (watch uses atomics).
        let _ = entry.seq_sender.send(new_seq);
    }

    /// Subscribe to a task's events for replay + live streaming.
    ///
    /// Returns `None` if the task doesn't exist.
    ///
    /// The `seq_sender.subscribe()` call happens BEFORE the caller reads
    /// `entry.events`, preserving the "no missed events" guarantee.
    pub fn subscribe(&self, task_id: &str) -> Option<EventSubscriber> {
        // Step 1 — read lock: clone Arc, release immediately.
        let entry = {
            let tasks = self.tasks.read().unwrap();
            tasks.get(task_id)?.clone()
        };

        // Step 2 — subscribe BEFORE any read of entry.events (ordering guarantee).
        let rx = entry.seq_sender.subscribe();

        Some((entry, rx))
    }

    /// List all tasks, newest first.
    ///
    /// The map read lock is held only long enough to collect `Vec<Arc<TaskEntry>>`.
    pub fn list_tasks(&self) -> Vec<TaskInfo> {
        // Step 1 — read lock: collect Arc refs, release immediately.
        let entries: Vec<Arc<TaskEntry>> = {
            let tasks = self.tasks.read().unwrap();
            tasks.values().cloned().collect()
        };

        // Step 2 — lock each entry's info briefly to clone it; no map lock held.
        let mut infos: Vec<TaskInfo> = entries
            .iter()
            .map(|e| e.info.lock().unwrap().clone())
            .collect();

        infos.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        infos
    }

    /// Get info for a single task.
    ///
    /// The map read lock is held only long enough to clone the `Arc<TaskEntry>`.
    pub fn get_task_info(&self, task_id: &str) -> Option<TaskInfo> {
        // Step 1 — read lock: clone Arc, release immediately.
        let entry = {
            let tasks = self.tasks.read().unwrap();
            tasks.get(task_id)?.clone()
        };

        // Step 2 — lock info briefly to clone it; no map lock held.
        let info = entry.info.lock().unwrap().clone();
        Some(info)
    }

    /// Cancel a running task by killing its process and publishing a `Failed` event.
    ///
    /// The map read lock is released before the kill function is called and before
    /// `publish_event` is invoked.
    pub fn cancel_task(&self, task_id: &str) {
        // Step 1 — read lock: clone Arc, release immediately.
        let entry = {
            let tasks = self.tasks.read().unwrap();
            match tasks.get(task_id) {
                Some(e) => e.clone(),
                None => return,
            }
        };

        // Step 2 — invoke kill function if present; no map lock held.
        {
            let kill_guard = entry.kill_fn.lock().unwrap();
            if let Some(ref kill_fn) = *kill_guard {
                kill_fn();
            }
        }

        // Step 3 — publish the terminal event; no map lock held.
        self.publish_event(task_id, TaskEvent::Failed("Cancelled by user.".to_string()));
    }

    /// Store a kill function for a running task (for cancellation purposes).
    ///
    /// The map read lock is held only long enough to clone the `Arc<TaskEntry>`.
    pub fn set_kill_fn<F: Fn() + Send + 'static>(&self, task_id: &str, kill_fn: F) {
        // Step 1 — read lock: clone Arc, release immediately.
        let entry = {
            let tasks = self.tasks.read().unwrap();
            match tasks.get(task_id) {
                Some(e) => e.clone(),
                None => return,
            }
        };

        // Step 2 — store the kill function; no map lock held.
        *entry.kill_fn.lock().unwrap() = Some(Box::new(kill_fn));
    }

    /// Return the `Arc<TaskEntry>` for a task (e.g. for progress extraction).
    ///
    /// The map read lock is held only long enough to clone the `Arc<TaskEntry>`.
    pub fn get_task_entry(&self, task_id: &str) -> Option<Arc<TaskEntry>> {
        let tasks = self.tasks.read().unwrap();
        tasks.get(task_id).cloned()
    }
}

/// The global task registry, shared across all requests.
///
/// No outer `Mutex` is needed — interior mutability is handled by the
/// `RwLock` inside `TaskRegistry`.
pub static TASK_REGISTRY: Lazy<Arc<TaskRegistry>> = Lazy::new(|| Arc::new(TaskRegistry::new()));

/// Convenience accessor for the global registry.
pub fn task_registry() -> Arc<TaskRegistry> {
    TASK_REGISTRY.clone()
}

/// Helper: publish an event to the global registry (no outer lock needed).
pub fn publish_event(task_id: &str, event: TaskEvent) {
    TASK_REGISTRY.publish_event(task_id, event);
}

/// Build an async stream that replays a task's event history and then streams
/// live events until a terminal event (`Completed` or `Failed`) is emitted.
///
/// This function includes a keep-alive mechanism to prevent HTTP connection
/// timeouts on mobile browsers. A heartbeat log event is sent every 2-3 seconds
/// if no real events are received, keeping the stream alive.
///
/// This function is safe to call from multiple clients simultaneously.
pub fn create_event_stream(
    task_id: &str,
) -> Option<impl futures::Stream<Item = Result<TaskEvent, anyhow::Error>>> {
    // subscribe() returns (Arc<TaskEntry>, watch::Receiver<usize>).
    // seq_sender.subscribe() is called inside subscribe() BEFORE any read of
    // entry.events, so no events can be missed between subscription and replay.
    let (entry, mut rx) = TASK_REGISTRY.subscribe(task_id)?;

    let (tx, stream_rx) = futures::channel::mpsc::unbounded::<Result<TaskEvent, anyhow::Error>>();

    tokio::spawn(async move {
        let mut cursor = 0usize;
        let keep_alive_interval = tokio::time::interval(std::time::Duration::from_secs(2));
        let mut keep_alive = keep_alive_interval;
        let mut last_event_time = std::time::Instant::now();

        loop {
            // Mark the current seq as "seen" so changed() only triggers for NEW events.
            rx.borrow_and_update();

            // Drain all events since cursor — no map lock held, only entry.events.
            let batch: Vec<TaskEvent> = {
                let events = entry.events.lock().unwrap();
                events[cursor..].to_vec()
            };

            let mut is_terminal = false;
            for event in batch {
                cursor += 1;
                let terminal = matches!(event, TaskEvent::Completed | TaskEvent::Failed(_));
                if tx.unbounded_send(Ok(event)).is_err() {
                    return; // Client disconnected
                }
                last_event_time = std::time::Instant::now();
                if terminal {
                    is_terminal = true;
                    break;
                }
            }

            if is_terminal {
                break;
            }

            // Race between waiting for new events and sending keep-alive heartbeats.
            tokio::select! {
                _ = rx.changed() => {
                    // New event is available.
                    continue;
                }
                _ = keep_alive.tick() => {
                    // Send a keep-alive heartbeat to prevent connection timeout
                    // (especially important for mobile browsers).
                    let msg = format!("[Keep-alive: stream active at {}]",
                        std::time::Instant::now()
                            .duration_since(last_event_time)
                            .as_secs());
                    let heartbeat = TaskEvent::Log(msg);
                    if tx.unbounded_send(Ok(heartbeat)).is_err() {
                        return; // Client disconnected
                    }
                }
            }
        }
    });

    Some(stream_rx)
}
