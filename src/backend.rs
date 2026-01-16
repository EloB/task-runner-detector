//! Backend thread for task registry, fuzzy search, and scanner integration

use crate::messages::{SearchRequest, SearchResponse, TaskItem};
use crate::registry::{Registry, Task};
use crate::{scan_streaming, ScanOptions, TaskRunner};
use nucleo::{Config, Nucleo, Utf32String};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, RwLock};

/// Shared task storage type
pub type SharedTasks = Arc<RwLock<Vec<TaskItem>>>;

/// Data stored in nucleo for each task - just the index
#[derive(Clone)]
struct TaskRef {
    index: u32,
}

/// Backend state and operations
pub struct Backend {
    /// The nucleo fuzzy matcher
    nucleo: Nucleo<TaskRef>,
    /// Shared task storage (read by UI)
    tasks: SharedTasks,
    /// Task registry for deduplication (backend-only)
    registry: Registry,
    /// Root path for folder display
    root: PathBuf,
    /// Current query
    current_query: String,
    /// Whether scanning is complete
    scanning_done: bool,
}

impl Backend {
    pub fn new(root: PathBuf, tasks: SharedTasks) -> Self {
        // Use multiple threads for parallel fuzzy matching
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let nucleo = Nucleo::new(Config::DEFAULT, Arc::new(|| {}), Some(num_threads), 1);

        Self {
            nucleo,
            tasks,
            registry: Registry::new(),
            root,
            current_query: String::new(),
            scanning_done: false,
        }
    }

    /// Main backend loop
    pub fn run(
        mut self,
        scanner_rx: Receiver<TaskRunner>,
        request_rx: Receiver<SearchRequest>,
        response_tx: Sender<SearchResponse>,
    ) {
        loop {
            // 1. Check for new search request (always take the latest)
            let mut pending_request: Option<SearchRequest> = None;
            loop {
                match request_rx.try_recv() {
                    Ok(request) => {
                        pending_request = Some(request);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }

            // 2. Drain tasks from scanner
            loop {
                match scanner_rx.try_recv() {
                    Ok(runner) => {
                        self.add_runner(runner);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.scanning_done = true;
                        break;
                    }
                }
            }

            // 3. Handle pending search request
            if let Some(request) = pending_request {
                let response = self.handle_search(request);
                if response_tx.send(response).is_err() {
                    return;
                }
            } else {
                self.nucleo.tick(10);
            }
        }
    }

    /// Add a task runner's tasks
    fn add_runner(&mut self, runner: TaskRunner) {
        let injector = self.nucleo.injector();

        for task in runner.tasks {
            let registry_task = Task {
                name: task.name.clone(),
                runner_type: runner.runner_type,
                config_path: runner.config_path.clone(),
            };

            let len_before = self.registry.len();
            self.registry.insert(registry_task.clone());

            // Only add if new (registry grew)
            if self.registry.len() > len_before {
                let folder = registry_task.folder_display(&self.root);

                let item = TaskItem {
                    folder: folder.clone(),
                    command: task.command.clone(),
                    script: task.script.clone(),
                    runner_type: runner.runner_type,
                    config_path: runner.config_path.clone(),
                };

                // Add to shared tasks
                let index = {
                    let mut tasks = self.tasks.write().unwrap();
                    let idx = tasks.len() as u32;
                    tasks.push(item);
                    idx
                };

                // Add to nucleo
                let search_text = format!("{} {}", folder, task.command);
                injector.push(TaskRef { index }, |_, cols| {
                    cols[0] = Utf32String::from(search_text.as_str());
                });
            }
        }
    }

    /// Calculate the correct scroll offset to make selected_index visible
    fn calculate_scroll_for_selected(
        &self,
        all_indices: &[u32],
        requested_offset: usize,
        selected_index: usize,
        viewport_lines: usize,
    ) -> usize {
        if all_indices.is_empty() || viewport_lines == 0 {
            return 0;
        }

        let selected_index = selected_index.min(all_indices.len().saturating_sub(1));
        let tasks = self.tasks.read().unwrap();

        // Helper to count headers for a task when it's at a given position in viewport
        let headers_for_task = |idx: usize, prev_idx: Option<usize>| -> usize {
            let folder = &tasks[all_indices[idx] as usize].folder;
            if let Some(prev) = prev_idx {
                let prev_folder = &tasks[all_indices[prev] as usize].folder;
                if prev_folder == folder {
                    0
                } else {
                    let prev_segs: Vec<&str> = if prev_folder == "." {
                        vec![]
                    } else {
                        prev_folder.split('/').collect()
                    };
                    let curr_segs: Vec<&str> = if folder == "." {
                        vec![]
                    } else {
                        folder.split('/').collect()
                    };
                    let common = prev_segs
                        .iter()
                        .zip(curr_segs.iter())
                        .take_while(|(a, b)| a == b)
                        .count();
                    curr_segs.len() - common
                }
            } else {
                // First task in viewport - count root + folder depth
                if folder == "." {
                    1
                } else {
                    1 + folder.split('/').count()
                }
            }
        };

        // Check if selected is visible from requested_offset
        let mut lines_used = 0;
        for i in requested_offset..all_indices.len() {
            let prev = if i == requested_offset {
                None
            } else {
                Some(i - 1)
            };
            let headers = headers_for_task(i, prev);
            let task_lines = headers + 1;

            if lines_used + task_lines > viewport_lines {
                break;
            }
            lines_used += task_lines;

            if i == selected_index {
                // Selected is visible from requested_offset
                return requested_offset;
            }
        }

        // Selected not visible - need to calculate new scroll
        if selected_index < requested_offset {
            // Selected is above viewport - put it at top
            return selected_index;
        }

        // Selected is below viewport - find smallest scroll that shows it
        // Start from requested_offset+1 and search forward until selected fits
        for try_scroll in (requested_offset + 1)..=selected_index {
            let mut lines_used = 0;
            let mut selected_fits = false;

            for i in try_scroll..=selected_index {
                let prev = if i == try_scroll { None } else { Some(i - 1) };
                let headers = headers_for_task(i, prev);
                let task_lines = headers + 1;

                if lines_used + task_lines > viewport_lines {
                    break;
                }
                lines_used += task_lines;

                if i == selected_index {
                    selected_fits = true;
                    break;
                }
            }

            if selected_fits {
                return try_scroll;
            }
        }

        selected_index // Fallback - show selected at top
    }

    /// Handle a search request
    fn handle_search(&mut self, req: SearchRequest) -> SearchResponse {
        // Update pattern if query changed
        if req.query != self.current_query {
            self.nucleo.pattern.reparse(
                0,
                &req.query,
                nucleo::pattern::CaseMatching::Ignore,
                nucleo::pattern::Normalization::Smart,
                false,
            );
            self.current_query = req.query.clone();
        }

        // Tick until matching is complete
        loop {
            let status = self.nucleo.tick(10);
            if !status.running {
                break;
            }
        }

        // Get results
        let snapshot = self.nucleo.snapshot();
        let matched_count = snapshot.matched_item_count();

        let matched_indices: Vec<u32> = if req.query.is_empty() {
            // No query - show all tasks sorted by folder/name
            self.registry
                .sorted_ids()
                .into_iter()
                .map(|id| id.0 as u32)
                .collect()
        } else {
            // With query - nucleo returns items sorted by score (best first)
            snapshot
                .matched_items(0..matched_count)
                .map(|item| item.data.index)
                .collect()
        };

        // Calculate corrected scroll offset
        let corrected_offset = self.calculate_scroll_for_selected(
            &matched_indices,
            req.offset,
            req.selected_index,
            req.viewport_lines,
        );

        // Return slice from corrected offset
        let total_tasks = self.tasks.read().unwrap().len();
        let matched_tasks = matched_indices.len();
        let start = corrected_offset.min(matched_tasks);
        let end = (corrected_offset + req.limit).min(matched_tasks);
        let sliced = matched_indices[start..end].to_vec();

        SearchResponse {
            matched_indices: sliced,
            offset: corrected_offset,
            total_tasks,
            matched_tasks,
            scanning_done: self.scanning_done,
        }
    }

    /// Add a runner for testing (bypasses channel)
    #[cfg(test)]
    pub fn add_runner_for_test(&mut self, runner: TaskRunner) {
        self.add_runner(runner);
        // Tick nucleo to process injected items
        for _ in 0..10 {
            self.nucleo.tick(10);
        }
    }

    /// Handle a search request for testing (bypasses channel)
    #[cfg(test)]
    pub fn handle_search_for_test(&mut self, request: SearchRequest) -> SearchResponse {
        self.scanning_done = true;
        self.handle_search(request)
    }
}

/// Spawn the backend thread
pub fn spawn_backend(
    root: PathBuf,
    options: ScanOptions,
    tasks: SharedTasks,
    request_rx: Receiver<SearchRequest>,
    response_tx: Sender<SearchResponse>,
) -> std::thread::JoinHandle<()> {
    let (scanner_tx, scanner_rx) = std::sync::mpsc::channel();
    let _scanner_handle = scan_streaming(root.clone(), options, scanner_tx);

    std::thread::spawn(move || {
        let backend = Backend::new(root, tasks);
        backend.run(scanner_rx, request_rx, response_tx);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunnerType;

    fn create_test_backend() -> (Backend, SharedTasks) {
        let tasks = Arc::new(RwLock::new(Vec::new()));
        let backend = Backend::new(PathBuf::from("/test"), tasks.clone());
        (backend, tasks)
    }

    #[test]
    fn test_backend_adds_tasks_to_shared_storage() {
        let (mut backend, tasks) = create_test_backend();

        backend.add_runner(TaskRunner {
            config_path: PathBuf::from("/test/package.json"),
            runner_type: RunnerType::Npm,
            tasks: vec![crate::Task {
                name: "build".to_string(),
                command: "npm run build".to_string(),
                description: None,
                script: None,
            }],
        });

        let tasks = tasks.read().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].command, "npm run build");
        assert_eq!(tasks[0].folder, ".");
    }

    #[test]
    fn test_backend_deduplicates_tasks() {
        let (mut backend, tasks) = create_test_backend();

        // Add same task twice
        for _ in 0..2 {
            backend.add_runner(TaskRunner {
                config_path: PathBuf::from("/test/package.json"),
                runner_type: RunnerType::Npm,
                tasks: vec![crate::Task {
                    name: "build".to_string(),
                    command: "npm run build".to_string(),
                    description: None,
                    script: None,
                }],
            });
        }

        let tasks = tasks.read().unwrap();
        assert_eq!(tasks.len(), 1); // Should be deduplicated
    }

    #[test]
    fn test_backend_search_returns_sorted_indices() {
        let (mut backend, tasks) = create_test_backend();

        backend.add_runner(TaskRunner {
            config_path: PathBuf::from("/test/b/package.json"),
            runner_type: RunnerType::Npm,
            tasks: vec![crate::Task {
                name: "test".to_string(),
                command: "npm test".to_string(),
                description: None,
                script: None,
            }],
        });

        backend.add_runner(TaskRunner {
            config_path: PathBuf::from("/test/a/package.json"),
            runner_type: RunnerType::Npm,
            tasks: vec![crate::Task {
                name: "build".to_string(),
                command: "npm run build".to_string(),
                description: None,
                script: None,
            }],
        });

        // Let nucleo process
        for _ in 0..10 {
            backend.nucleo.tick(10);
        }
        backend.scanning_done = true;

        let response = backend.handle_search(SearchRequest {
            query: String::new(),
            offset: 0,
            limit: 100,
            viewport_lines: 30,
            selected_index: 0,
        });

        // Should be sorted by folder: a before b
        assert_eq!(response.matched_indices.len(), 2);
        let tasks = tasks.read().unwrap();
        let first_folder = &tasks[response.matched_indices[0] as usize].folder;
        let second_folder = &tasks[response.matched_indices[1] as usize].folder;
        assert!(first_folder < second_folder);
    }
}
