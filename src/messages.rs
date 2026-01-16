//! Message types for UI/Backend communication

use crate::registry::TaskId;
use crate::RunnerType;
use std::path::PathBuf;

/// Request from UI to Backend for search results
#[derive(Debug, Clone)]
pub struct SearchRequest {
    /// Fuzzy search query
    pub query: String,
    /// Starting index for viewport slice
    pub offset: usize,
    /// Number of items to return
    pub limit: usize,
    /// Viewport height in lines
    pub viewport_lines: usize,
    /// Currently selected task index
    pub selected_index: usize,
}

/// Response from Backend to UI with search results
#[derive(Debug, Clone)]
pub struct SearchResponse {
    /// Matched task indices (sorted by folder, then runner type, then name)
    /// This is a slice starting at the corrected offset
    pub matched_indices: Vec<u32>,
    /// Corrected scroll offset that ensures selected task is visible
    pub offset: usize,
    /// Total number of tasks in registry
    pub total_tasks: usize,
    /// Number of tasks matching the current query
    pub matched_tasks: usize,
    /// Whether scanning is complete
    pub scanning_done: bool,
}

/// Task item stored in shared storage
#[derive(Debug, Clone)]
pub struct TaskItem {
    pub id: TaskId,
    pub folder: String,
    pub name: String,
    pub command: String,
    pub script: Option<String>,
    pub runner_type: RunnerType,
    pub config_path: PathBuf,
}

impl TaskItem {
    /// Get the runner icon for this task
    pub fn runner_icon(&self) -> &'static str {
        self.runner_type.icon()
    }
}

/// Full task information for the selected task (used when running)
#[derive(Debug, Clone)]
pub struct SelectedTask {
    #[allow(dead_code)]
    pub id: TaskId,
    #[allow(dead_code)]
    pub name: String,
    pub command: String,
    pub script: Option<String>,
    pub runner_type: RunnerType,
    pub config_path: PathBuf,
}

impl From<&TaskItem> for SelectedTask {
    fn from(item: &TaskItem) -> Self {
        Self {
            id: item.id,
            name: item.name.clone(),
            command: item.command.clone(),
            script: item.script.clone(),
            runner_type: item.runner_type,
            config_path: item.config_path.clone(),
        }
    }
}
