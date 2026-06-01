//! Global task registry for tracking long-running background operations.
//!
//! Tasks are stored in a global registry. The client polls for new events
//! using a monotonically increasing cursor, which is safe and works on all
//! platforms (no SSE/long-lived connections required).

use colmap_openmvs_api::{TaskEvent, TaskEventBatch, TaskId, TaskInfo, TaskKind, TaskState};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// Maximum number of events buffered per task (oldest are dropped if exceeded).
const MAX_EVENTS: usize = 100_000;

struct TaskEntry {
    /// Task metadata (state, timestamps, etc.)
    pub info: Mutex<TaskInfo>,
    /// Append-only event log.
    pub events: Mutex<Vec<TaskEvent>>,
    /// Optional kill function for the running task.
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
    pub fn create_task(&self, kind: TaskKind, context_key: String) -> TaskId {
        let id = uuid::Uuid::new_v4().to_string();
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
            kill_fn: Mutex::new(None),
        });

        self.tasks.write().unwrap().insert(id.clone(), entry);
        id
    }

    /// Publish an event to a task. Updates task state for terminal events.
    pub fn publish_event(&self, task_id: &str, event: TaskEvent) {
        let entry = {
            let tasks = self.tasks.read().unwrap();
            match tasks.get(task_id) {
                Some(e) => e.clone(),
                None => return,
            }
        };

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

        {
            let mut events = entry.events.lock().unwrap();
            events.push(event);
            if events.len() > MAX_EVENTS {
                let drop_count = events.len() - MAX_EVENTS;
                events.drain(0..drop_count);
            }
        }
    }

    /// Poll for new events since `cursor`. Returns `None` if task not found.
    pub fn poll_events(&self, task_id: &str, cursor: usize) -> Option<TaskEventBatch> {
        let entry = {
            let tasks = self.tasks.read().unwrap();
            tasks.get(task_id)?.clone()
        };

        let events = entry.events.lock().unwrap();
        let start = cursor.min(events.len());
        let new_events: Vec<TaskEvent> = events[start..].to_vec();
        let new_cursor = cursor + new_events.len();
        drop(events);

        let is_terminal = new_events
            .iter()
            .any(|e| matches!(e, TaskEvent::Completed | TaskEvent::Failed(_)));

        Some(TaskEventBatch {
            events: new_events,
            cursor: new_cursor,
            is_terminal,
            task_found: true,
        })
    }

    /// List all tasks, newest first.
    pub fn list_tasks(&self) -> Vec<TaskInfo> {
        let entries: Vec<Arc<TaskEntry>> = {
            let tasks = self.tasks.read().unwrap();
            tasks.values().cloned().collect()
        };

        let mut infos: Vec<TaskInfo> = entries
            .iter()
            .map(|e| e.info.lock().unwrap().clone())
            .collect();

        infos.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        infos
    }

    /// Get info for a single task.
    pub fn get_task_info(&self, task_id: &str) -> Option<TaskInfo> {
        let entry = {
            let tasks = self.tasks.read().unwrap();
            tasks.get(task_id)?.clone()
        };

        let info = entry.info.lock().unwrap().clone();
        Some(info)
    }

    /// Return the latest stage progress event recorded for a task.
    pub fn latest_pipeline_progress(&self, task_id: &str) -> Option<f32> {
        let entry = {
            let tasks = self.tasks.read().unwrap();
            tasks.get(task_id)?.clone()
        };

        let events = entry.events.lock().unwrap();
        events.iter().rev().find_map(|event| {
            if let TaskEvent::PipelineStageProgress { progress, .. } = event {
                Some(*progress)
            } else {
                None
            }
        })
    }

    /// Cancel a running task.
    pub fn cancel_task(&self, task_id: &str) {
        let entry = {
            let tasks = self.tasks.read().unwrap();
            match tasks.get(task_id) {
                Some(e) => e.clone(),
                None => return,
            }
        };

        {
            let kill_guard = entry.kill_fn.lock().unwrap();
            if let Some(ref kill_fn) = *kill_guard {
                kill_fn();
            }
        }

        self.publish_event(task_id, TaskEvent::Failed("Cancelled by user.".to_string()));
    }

    /// Store a kill function for a running task.
    pub fn set_kill_fn<F: Fn() + Send + 'static>(&self, task_id: &str, kill_fn: F) {
        let entry = {
            let tasks = self.tasks.read().unwrap();
            match tasks.get(task_id) {
                Some(e) => e.clone(),
                None => return,
            }
        };

        *entry.kill_fn.lock().unwrap() = Some(Box::new(kill_fn));
    }
}

/// The global task registry.
pub static TASK_REGISTRY: Lazy<Arc<TaskRegistry>> = Lazy::new(|| Arc::new(TaskRegistry::new()));

/// Convenience accessor for the global registry.
pub fn task_registry() -> Arc<TaskRegistry> {
    TASK_REGISTRY.clone()
}

/// Helper: publish an event to the global registry.
pub fn publish_event(task_id: &str, event: TaskEvent) {
    TASK_REGISTRY.publish_event(task_id, event);
}
