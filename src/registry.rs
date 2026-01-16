//! Task registry for storing and looking up tasks

use crate::RunnerType;
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Composite key for task deduplication and sorting
/// Uses \x00 separator between folder and command so parent tasks sort before child folders
/// e.g., "fixtures\x00build" < "fixtures/apps\x00build" (because \x00 < '/')
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskKey(String);

impl TaskKey {
    pub fn new(config_path: &Path, runner_type: RunnerType, name: &str) -> Self {
        let folder = config_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        // Sort by folder, then runner display name, then task name
        // Use \x00 as separator so parent tasks sort before child folders
        Self(format!(
            "{}\x00{}\x00{}",
            folder,
            runner_type.display_name(),
            name
        ))
    }
}

impl Borrow<str> for TaskKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// Unique identifier for a task (index into tasks Vec)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub usize);

/// A task stored in the registry for deduplication
#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    pub runner_type: RunnerType,
    pub config_path: PathBuf,
}

impl Task {
    /// Get the folder path relative to root for display
    pub fn folder_display(&self, root: &Path) -> String {
        let relative = self
            .config_path
            .strip_prefix(root)
            .unwrap_or(&self.config_path);
        let path_str = relative.to_string_lossy();

        if !path_str.contains('/') && !path_str.contains('\\') {
            ".".to_string()
        } else {
            relative
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| ".".to_string())
        }
    }
}

/// Task registry with deduplication and folder grouping
pub struct Registry {
    /// BTreeMap index for dedup + lookup by key
    index: BTreeMap<TaskKey, TaskId>,
    /// Append-only storage of tasks
    tasks: Vec<Task>,
    /// Task IDs grouped by folder path
    by_folder: BTreeMap<PathBuf, Vec<TaskId>>,
    /// Folder discovery order (preserved for deterministic rendering)
    folder_order: Vec<PathBuf>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            index: BTreeMap::new(),
            tasks: Vec::new(),
            by_folder: BTreeMap::new(),
            folder_order: Vec::new(),
        }
    }

    /// Insert a task, returning its ID. Returns existing ID if duplicate.
    pub fn insert(&mut self, task: Task) -> TaskId {
        let key = TaskKey::new(&task.config_path, task.runner_type, &task.name);

        // Check for existing task with same key
        if let Some(&existing) = self.index.get(&key) {
            return existing;
        }

        let id = TaskId(self.tasks.len());
        let folder = task
            .config_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        // Track folder discovery order
        if !self.by_folder.contains_key(&folder) {
            self.folder_order.push(folder.clone());
        }

        self.by_folder.entry(folder).or_default().push(id);
        self.tasks.push(task);
        self.index.insert(key, id);
        id
    }

    /// Total number of tasks
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Get all task IDs in sorted order (by folder/runner/name)
    pub fn sorted_ids(&self) -> Vec<TaskId> {
        self.index.values().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_insert() {
        let mut registry = Registry::new();

        let task = Task {
            name: "build".to_string(),
            runner_type: RunnerType::Npm,
            config_path: PathBuf::from("/project/package.json"),
        };

        let id = registry.insert(task);
        assert_eq!(id, TaskId(0));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_registry_dedup() {
        let mut registry = Registry::new();

        let task1 = Task {
            name: "build".to_string(),
            runner_type: RunnerType::Npm,
            config_path: PathBuf::from("/project/package.json"),
        };

        let task2 = Task {
            name: "build".to_string(),
            runner_type: RunnerType::Npm,
            config_path: PathBuf::from("/project/package.json"),
        };

        let id1 = registry.insert(task1);
        let id2 = registry.insert(task2);

        assert_eq!(id1, id2);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_sorted_ids() {
        let mut registry = Registry::new();

        // Insert in reverse order
        registry.insert(Task {
            name: "build".to_string(),
            runner_type: RunnerType::Npm,
            config_path: PathBuf::from("/project/b/package.json"),
        });

        registry.insert(Task {
            name: "test".to_string(),
            runner_type: RunnerType::Npm,
            config_path: PathBuf::from("/project/a/package.json"),
        });

        let sorted = registry.sorted_ids();
        assert_eq!(sorted.len(), 2);
        // "a" folder should come before "b" folder
        assert_eq!(sorted[0], TaskId(1));
        assert_eq!(sorted[1], TaskId(0));
    }
}
