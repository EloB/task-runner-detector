//! Task CLI - Discover and run tasks from various config files
//!
//! Usage:
//!   task                    # Interactive picker (scan cwd, select, run)
//!   task <path>             # Interactive picker for specific directory
//!   task -j                 # JSON output
//!   task -s                 # Streaming NDJSON output
//!   task -j -q "query"      # Filter JSON output with fuzzy search
//!   task -i                 # Include files ignored by .gitignore

use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{self, stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use clap::Parser;
use console::style;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Matcher,
};

use task_runner_detector::{
    scan_streaming, scan_with_options, RunnerType, ScanOptions, Task, TaskRunner,
};

#[derive(Parser)]
#[command(name = "task")]
#[command(about = "Discover and run tasks from various config files")]
#[command(version)]
struct Cli {
    /// Output results as JSON array (waits for scan to complete)
    #[arg(short = 'j', long)]
    json: bool,

    /// Output results as streaming NDJSON (one JSON object per line)
    #[arg(short = 's', long)]
    json_stream: bool,

    /// Filter tasks using fuzzy search (works with --json and --json-stream)
    #[arg(short = 'q', long)]
    query: Option<String>,

    /// Don't respect .gitignore and scan all files
    #[arg(short = 'i', long)]
    no_ignore: bool,

    /// Directory to scan (defaults to current directory)
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,
}

/// A task with its source information for display
#[derive(Clone)]
struct DisplayTask {
    runner: RunnerType,
    config_path: PathBuf,
    task: Task,
}

/// Unique identifier for a task (used for stable selection and as HashMap key)
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct TaskId {
    config_path: PathBuf,
    task_name: String,
}

impl TaskId {
    fn from_task(task: &DisplayTask) -> Self {
        Self {
            config_path: task.config_path.clone(),
            task_name: task.task.name.clone(),
        }
    }
}

/// Application mode
#[derive(Clone, PartialEq)]
enum Mode {
    Select,
    Edit,
    Expanded,
}

/// An item in the picker - either a folder header or a task
#[derive(Clone)]
enum PickerItem {
    Folder {
        name: String,
        path: String,
        is_root: bool,
        depth: usize,
    },
    Task {
        task_id: TaskId,
        depth: usize,
    },
}

/// Normalized application state
/// - `tasks_by_id`: HashMap for O(1) lookups
/// - `task_ids`: Vec preserving insertion order
/// - `folder_ids`: Vec preserving folder discovery order
#[derive(Clone)]
struct AppState {
    // Normalized task storage
    tasks_by_id: HashMap<TaskId, DisplayTask>,
    task_ids: Vec<TaskId>, // Maintains insertion order

    // Folder discovery order
    folder_ids: Vec<String>,

    // Scan status
    scanning_done: bool,

    // User interaction
    query: String,
    query_cursor: usize,
    selected_task: Option<TaskId>,
    mode: Mode,
    edit_buffer: String,
    edit_cursor: usize,

    // Viewport
    scroll_offset: usize,
}

impl AppState {
    /// Get a task by ID (O(1) lookup)
    fn get_task(&self, id: &TaskId) -> Option<&DisplayTask> {
        self.tasks_by_id.get(id)
    }

    /// Number of tasks
    fn task_count(&self) -> usize {
        self.task_ids.len()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            tasks_by_id: HashMap::new(),
            task_ids: Vec::new(),
            folder_ids: Vec::new(),
            scanning_done: false,
            query: String::new(),
            query_cursor: 0,
            selected_task: None,
            mode: Mode::Select,
            edit_buffer: String::new(),
            edit_cursor: 0,
            scroll_offset: 0,
        }
    }
}

struct DerivedState {
    filtered: Vec<PickerItem>,
    selected_index: usize,
    folder_indices: HashMap<TaskId, Vec<u32>>, // Folder highlight indices per task
    cmd_indices: HashMap<TaskId, Vec<u32>>,    // Command highlight indices per task
}

enum AppEvent {
    TasksReceived {
        tasks: Vec<DisplayTask>,
        folders: Vec<String>,
    },
    ScanComplete,
    Key(KeyEvent),
}

/// Result from the picker
struct PickerResult {
    task: DisplayTask,
    command: String,
}

/// Get depth of a picker item
fn item_depth(item: &PickerItem) -> usize {
    match item {
        PickerItem::Folder { depth, .. } => *depth,
        PickerItem::Task { depth, .. } => *depth,
    }
}

/// Result from update - either continue or exit
enum UpdateResult {
    Continue(AppState),
    Exit(Option<PickerResult>),
}

/// Get folder key from a config path relative to root
fn folder_key(config_path: &Path, root: &Path) -> String {
    let relative = config_path.strip_prefix(root).unwrap_or(config_path);
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

/// Derive picker items from normalized state
fn derive_items(state: &AppState, root: &Path) -> Vec<PickerItem> {
    // Group task IDs by folder, maintaining folder discovery order
    let mut folder_tasks: Vec<(String, Vec<TaskId>)> = Vec::new();
    let mut folder_index: HashMap<String, usize> = HashMap::new();

    // Initialize folders in discovery order
    for folder in &state.folder_ids {
        folder_index.insert(folder.clone(), folder_tasks.len());
        folder_tasks.push((folder.clone(), Vec::new()));
    }

    // Assign tasks to folders by iterating over task_ids (preserves order)
    for task_id in &state.task_ids {
        if let Some(task) = state.get_task(task_id) {
            let folder = folder_key(&task.config_path, root);
            if let Some(&idx) = folder_index.get(&folder) {
                folder_tasks[idx].1.push(task_id.clone());
            } else {
                // Folder wasn't in folder_ids, add it now
                folder_index.insert(folder.clone(), folder_tasks.len());
                folder_tasks.push((folder, vec![task_id.clone()]));
            }
        }
    }

    // Sort tasks within each folder by runner name, then task name
    for (_, task_ids) in &mut folder_tasks {
        task_ids.sort_by(|a, b| {
            let ta = state.get_task(a);
            let tb = state.get_task(b);
            match (ta, tb) {
                (Some(ta), Some(tb)) => ta
                    .runner
                    .display_name()
                    .cmp(tb.runner.display_name())
                    .then_with(|| ta.task.name.cmp(&tb.task.name)),
                _ => std::cmp::Ordering::Equal,
            }
        });
    }

    // Build tree structure
    #[derive(Default)]
    struct FolderNode {
        task_ids: Vec<TaskId>,
        children: Vec<(String, FolderNode)>,
    }

    let mut tree = FolderNode::default();

    for (path, task_ids) in folder_tasks {
        if path == "." {
            tree.task_ids = task_ids;
        } else {
            let parts: Vec<&str> = path.split('/').collect();
            let mut current = &mut tree;

            for part in parts {
                let child_idx = current.children.iter().position(|(name, _)| name == part);

                if let Some(idx) = child_idx {
                    current = &mut current.children[idx].1;
                } else {
                    current
                        .children
                        .push((part.to_string(), FolderNode::default()));
                    let last_idx = current.children.len() - 1;
                    current = &mut current.children[last_idx].1;
                }
            }
            current.task_ids = task_ids;
        }
    }

    // Flatten tree into picker items
    fn flatten(
        node: &FolderNode,
        name: &str,
        path: &str,
        depth: usize,
        is_root: bool,
        items: &mut Vec<PickerItem>,
    ) {
        items.push(PickerItem::Folder {
            name: name.to_string(),
            path: path.to_string(),
            is_root,
            depth,
        });

        for task_id in &node.task_ids {
            items.push(PickerItem::Task {
                task_id: task_id.clone(),
                depth: depth + 1,
            });
        }

        // Sort children alphabetically for deterministic ordering
        let mut sorted_children: Vec<_> = node.children.iter().collect();
        sorted_children.sort_by(|(a, _), (b, _)| a.cmp(b));

        for (child_name, child_node) in sorted_children {
            let child_path = if path.is_empty() {
                child_name.clone()
            } else {
                format!("{}/{}", path, child_name)
            };
            flatten(child_node, child_name, &child_path, depth + 1, false, items);
        }
    }

    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    let mut items = Vec::new();
    flatten(&tree, &root_name, "", 0, true, &mut items);
    items
}

/// Matching result with score and highlight indices
struct MatchedTask {
    task_id: TaskId,
    score: u32,
    folder_indices: Vec<u32>, // Matched chars in folder path
    cmd_indices: Vec<u32>,    // Matched chars in command
}

/// Compute matching tasks using nucleo fuzzy matcher
///
/// Supports fzf-like query syntax (handled by nucleo's Pattern::parse):
/// - `foo`   → fuzzy match
/// - `'foo`  → exact substring match
/// - `^foo`  → prefix match
/// - `foo$`  → suffix match
/// - `!foo`  → inverse match
/// - Multiple terms can be combined with spaces
fn compute_matching_tasks(
    items: &[PickerItem],
    state: &AppState,
    query: &str,
    root: &Path,
    matcher: &mut Matcher,
) -> Vec<MatchedTask> {
    if query.is_empty() {
        return items
            .iter()
            .filter_map(|item| {
                if let PickerItem::Task { task_id, .. } = item {
                    Some(MatchedTask {
                        task_id: task_id.clone(),
                        score: 0,
                        folder_indices: vec![],
                        cmd_indices: vec![],
                    })
                } else {
                    None
                }
            })
            .collect();
    }

    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);

    let mut matches: Vec<MatchedTask> = items
        .iter()
        .filter_map(|item| {
            if let PickerItem::Task { task_id, .. } = item {
                if let Some(task) = state.get_task(task_id) {
                    let folder = folder_key(&task.config_path, root);
                    let folder_len = folder.len();
                    let search_text = format!("{} {}", folder, task.task.command);
                    let mut buf = Vec::new();
                    let haystack = nucleo_matcher::Utf32Str::new(&search_text, &mut buf);
                    let mut indices = Vec::new();
                    pattern
                        .indices(haystack, matcher, &mut indices)
                        .map(|score| {
                            // Split indices into folder and command parts
                            let folder_indices: Vec<u32> = indices
                                .iter()
                                .filter(|&&i| (i as usize) < folder_len)
                                .copied()
                                .collect();
                            let cmd_indices: Vec<u32> = indices
                                .iter()
                                .filter(|&&i| i as usize > folder_len) // > to skip the space
                                .map(|&i| i - folder_len as u32 - 1) // -1 for the space
                                .collect();
                            MatchedTask {
                                task_id: task_id.clone(),
                                score,
                                folder_indices,
                                cmd_indices,
                            }
                        })
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    matches.sort_by(|a, b| b.score.cmp(&a.score));
    matches
}

/// Derive folder paths that contain matching tasks (pure)
fn derive_matching_folders(
    matching_tasks: &[MatchedTask],
    state: &AppState,
    root: &Path,
) -> HashSet<String> {
    let ancestor_paths = matching_tasks.iter().flat_map(|m| {
        let task_id = &m.task_id;
        if let Some(task) = state.get_task(task_id) {
            let folder = folder_key(&task.config_path, root);
            if folder == "." {
                vec![]
            } else {
                // Generate all ancestor paths: "a/b/c" -> ["a", "a/b", "a/b/c"]
                folder
                    .split('/')
                    .scan(String::new(), |acc, part| {
                        if acc.is_empty() {
                            *acc = part.to_string();
                        } else {
                            acc.push('/');
                            acc.push_str(part);
                        }
                        Some(acc.clone())
                    })
                    .collect::<Vec<_>>()
            }
        } else {
            vec![]
        }
    });

    std::iter::once(String::new()) // root always included
        .chain(ancestor_paths)
        .collect()
}

/// Derive filtered items from matching tasks, keeping tree structure
fn derive_filtered(
    matching_tasks: &[MatchedTask],
    matching_folders: &HashSet<String>,
    state: &AppState,
    root: &Path,
) -> Vec<PickerItem> {
    let matching_ids: HashSet<_> = matching_tasks.iter().map(|m| &m.task_id).collect();
    derive_items(state, root)
        .into_iter()
        .filter(|item| match item {
            PickerItem::Folder { path, is_root, .. } => *is_root || matching_folders.contains(path),
            PickerItem::Task { task_id, .. } => matching_ids.contains(task_id),
        })
        .collect()
}

/// Derive selected index from selected task identity
fn derive_selected_index(selected_task: Option<&TaskId>, filtered: &[PickerItem]) -> usize {
    if let Some(selected_id) = selected_task {
        for (i, item) in filtered.iter().enumerate() {
            if let PickerItem::Task { task_id, .. } = item {
                if task_id == selected_id {
                    return i;
                }
            }
        }
    }
    // Default to first task
    filtered
        .iter()
        .position(|i| matches!(i, PickerItem::Task { .. }))
        .unwrap_or(0)
}

/// Derive scroll offset to keep selection visible
fn derive_scroll(selected: usize, current_scroll: usize, visible_height: usize) -> usize {
    if selected < current_scroll {
        selected
    } else if selected >= current_scroll + visible_height {
        selected.saturating_sub(visible_height) + 1
    } else {
        current_scroll
    }
}

/// Derive all state at once (pure - matching done externally)
fn derive_all(state: &AppState, root: &Path, matching_tasks: &[MatchedTask]) -> DerivedState {
    let matching_folders = derive_matching_folders(matching_tasks, state, root);
    let filtered = derive_filtered(matching_tasks, &matching_folders, state, root);
    let selected_index = derive_selected_index(state.selected_task.as_ref(), &filtered);
    let folder_indices: HashMap<TaskId, Vec<u32>> = matching_tasks
        .iter()
        .map(|m| (m.task_id.clone(), m.folder_indices.clone()))
        .collect();
    let cmd_indices: HashMap<TaskId, Vec<u32>> = matching_tasks
        .iter()
        .map(|m| (m.task_id.clone(), m.cmd_indices.clone()))
        .collect();

    DerivedState {
        filtered,
        selected_index,
        folder_indices,
        cmd_indices,
    }
}

/// Handle a key event, returning new state or exit
fn handle_key(state: AppState, key: KeyEvent, derived: &DerivedState) -> UpdateResult {
    match key.code {
        // Ctrl+C always exits
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            UpdateResult::Exit(None)
        }

        // Escape: go back one step (Expanded → Edit → Select → Exit)
        KeyCode::Esc => {
            match state.mode {
                Mode::Expanded => {
                    // Go back to Edit with original command
                    let command = derived
                        .filtered
                        .get(derived.selected_index)
                        .and_then(|item| {
                            if let PickerItem::Task { task_id, .. } = item {
                                state.get_task(task_id)
                            } else {
                                None
                            }
                        })
                        .map(|t| t.task.command.clone())
                        .unwrap_or_default();
                    UpdateResult::Continue(AppState {
                        mode: Mode::Edit,
                        edit_buffer: command.clone(),
                        edit_cursor: command.len(),
                        ..state
                    })
                }
                Mode::Edit => UpdateResult::Continue(AppState {
                    mode: Mode::Select,
                    ..state
                }),
                Mode::Select => UpdateResult::Exit(None),
            }
        }

        // Run selected task
        KeyCode::Enter => {
            if let Some(PickerItem::Task { task_id, .. }) =
                derived.filtered.get(derived.selected_index)
            {
                if let Some(task) = state.get_task(task_id) {
                    let task = task.clone();
                    let command = if matches!(state.mode, Mode::Edit | Mode::Expanded) {
                        state.edit_buffer.clone()
                    } else {
                        task.task.command.clone()
                    };
                    return UpdateResult::Exit(Some(PickerResult { task, command }));
                }
            }
            UpdateResult::Continue(state)
        }

        // Tab: cycle through modes (Select → Edit → Expanded → Select)
        KeyCode::Tab => {
            let task = derived
                .filtered
                .get(derived.selected_index)
                .and_then(|item| {
                    if let PickerItem::Task { task_id, .. } = item {
                        state.get_task(task_id)
                    } else {
                        None
                    }
                });

            match state.mode {
                Mode::Select => {
                    if let Some(task) = task {
                        let command = task.task.command.clone();
                        UpdateResult::Continue(AppState {
                            mode: Mode::Edit,
                            edit_buffer: command.clone(),
                            edit_cursor: command.len(),
                            ..state
                        })
                    } else {
                        UpdateResult::Continue(state)
                    }
                }
                Mode::Edit => {
                    if let Some(task) = task {
                        if let Some(script) = &task.task.script {
                            return UpdateResult::Continue(AppState {
                                mode: Mode::Expanded,
                                edit_buffer: script.clone(),
                                edit_cursor: script.len(),
                                ..state
                            });
                        }
                    }
                    // No script to expand, stay in Edit mode
                    UpdateResult::Continue(state)
                }
                Mode::Expanded => {
                    // Tab disabled in expanded mode for now
                    UpdateResult::Continue(state)
                }
            }
        }

        // Navigation
        KeyCode::Up => {
            let new_idx = move_selection(derived.selected_index, &derived.filtered, -1);
            UpdateResult::Continue(AppState {
                mode: Mode::Select,
                selected_task: get_task_id_at(new_idx, &derived.filtered),
                ..state
            })
        }
        KeyCode::Down => {
            let new_idx = move_selection(derived.selected_index, &derived.filtered, 1);
            UpdateResult::Continue(AppState {
                mode: Mode::Select,
                selected_task: get_task_id_at(new_idx, &derived.filtered),
                ..state
            })
        }

        // Text input
        _ => {
            if matches!(state.mode, Mode::Edit | Mode::Expanded) {
                let (new_buffer, new_cursor) =
                    apply_input_event(&state.edit_buffer, state.edit_cursor, key);
                UpdateResult::Continue(AppState {
                    edit_buffer: new_buffer,
                    edit_cursor: new_cursor,
                    ..state
                })
            } else {
                let (new_query, new_cursor) =
                    apply_input_event(&state.query, state.query_cursor, key);
                // Reset selection when query changes
                let query_changed = new_query != state.query;
                let new_selected_task = if query_changed {
                    None
                } else {
                    state.selected_task
                };
                let new_scroll = if query_changed {
                    0
                } else {
                    state.scroll_offset
                };
                UpdateResult::Continue(AppState {
                    query: new_query,
                    query_cursor: new_cursor,
                    selected_task: new_selected_task,
                    scroll_offset: new_scroll,
                    ..state
                })
            }
        }
    }
}

/// Move selection, skipping folders, with wrap-around
fn move_selection(current: usize, filtered: &[PickerItem], delta: isize) -> usize {
    let len = filtered.len();
    if len == 0 {
        return current;
    }

    let mut pos = current;
    for _ in 0..len {
        // Wrap around using modulo
        pos = ((pos as isize + delta).rem_euclid(len as isize)) as usize;
        if matches!(filtered[pos], PickerItem::Task { .. }) {
            return pos;
        }
    }

    current
}

/// Get TaskId at a filtered index
fn get_task_id_at(index: usize, filtered: &[PickerItem]) -> Option<TaskId> {
    if let PickerItem::Task { task_id, .. } = filtered.get(index)? {
        Some(task_id.clone())
    } else {
        None
    }
}

/// Apply a key event to a text buffer, returning new buffer and cursor
fn apply_input_event(buffer: &str, cursor: usize, key: KeyEvent) -> (String, usize) {
    let chars: Vec<char> = buffer.chars().collect();
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let word_mod = key
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char('a') if ctrl => (buffer.to_string(), 0),
        KeyCode::Char('e') if ctrl => (buffer.to_string(), buffer.len()),
        KeyCode::Char('u') if ctrl => (chars[cursor..].iter().collect(), 0),
        KeyCode::Char('k') if ctrl => (chars[..cursor].iter().collect(), cursor),
        KeyCode::Char('w') if ctrl => {
            let before: String = chars[..cursor].iter().collect();
            let trimmed = before.trim_end();
            let new_pos = trimmed
                .rfind(char::is_whitespace)
                .map(|i| i + 1)
                .unwrap_or(0);
            (
                format!(
                    "{}{}",
                    &trimmed[..new_pos],
                    chars[cursor..].iter().collect::<String>()
                ),
                new_pos,
            )
        }
        KeyCode::Left if word_mod => {
            let mut p = cursor;
            while p > 0 && chars[p - 1].is_whitespace() {
                p -= 1;
            }
            while p > 0 && !chars[p - 1].is_whitespace() {
                p -= 1;
            }
            (buffer.to_string(), p)
        }
        KeyCode::Right if word_mod => {
            let mut p = cursor;
            while p < chars.len() && !chars[p].is_whitespace() {
                p += 1;
            }
            while p < chars.len() && chars[p].is_whitespace() {
                p += 1;
            }
            (buffer.to_string(), p)
        }
        KeyCode::Left => (buffer.to_string(), cursor.saturating_sub(1)),
        KeyCode::Right => (buffer.to_string(), (cursor + 1).min(chars.len())),
        KeyCode::Home => (buffer.to_string(), 0),
        KeyCode::End => (buffer.to_string(), chars.len()),
        KeyCode::Backspace if cursor > 0 => {
            let mut c = chars;
            c.remove(cursor - 1);
            (c.into_iter().collect(), cursor - 1)
        }
        KeyCode::Delete if cursor < chars.len() => {
            let mut c = chars;
            c.remove(cursor);
            (c.into_iter().collect(), cursor)
        }
        KeyCode::Char(ch) => {
            let mut c = chars;
            c.insert(cursor, ch);
            (c.into_iter().collect(), cursor + 1)
        }
        _ => (buffer.to_string(), cursor),
    }
}

/// Render the entire UI to a string
fn render(state: &AppState, derived: &DerivedState, root: &Path, terminal_height: usize) -> String {
    let mut output = String::new();

    // Header
    output.push_str("\x1b[36m  Task Runner Detector\x1b[0m");
    if !state.scanning_done {
        output.push_str(" \x1b[33m(scanning...)\x1b[0m");
    }
    output.push_str("\x1b[K\r\n");
    output.push_str(&format!(
        "\x1b[90m  {} tasks found\x1b[0m\x1b[K\r\n",
        state.task_count()
    ));
    output.push_str("\x1b[K\r\n");

    // Input line
    let (input_before, input_char, input_after) = render_input_cursor(
        &state.query,
        if state.mode == Mode::Select {
            state.query_cursor
        } else {
            state.query.len()
        },
    );

    if state.mode == Mode::Select {
        output.push_str(&format!(
            "\x1b[36m❯ \x1b[0m{}\x1b[7m{}\x1b[0m{}\x1b[K\r\n",
            input_before, input_char, input_after
        ));
    } else {
        output.push_str(&format!("\x1b[90m❯ {}\x1b[0m\x1b[K\r\n", state.query));
    }
    output.push_str("\x1b[K\r\n");

    // Calculate visible area
    let base_list_height = terminal_height.saturating_sub(8);

    // Find sticky ancestors for scroll context
    let sticky_ancestors = find_sticky_ancestors(&derived.filtered, state.scroll_offset);
    let sticky_count = sticky_ancestors.len();
    let list_height = base_list_height.saturating_sub(sticky_count);

    // Adjust scroll offset
    let scroll_offset = derive_scroll(derived.selected_index, state.scroll_offset, list_height);

    // Recalculate sticky after scroll adjustment
    let sticky_ancestors = find_sticky_ancestors(&derived.filtered, scroll_offset);
    let sticky_count = sticky_ancestors.len();
    let list_height = base_list_height.saturating_sub(sticky_count);

    // Render sticky ancestors
    for (filtered_idx, ancestor) in &sticky_ancestors {
        if let PickerItem::Folder { name, is_root, .. } = ancestor {
            if *is_root {
                output.push_str(&format!("  📁 \x1b[1;37m{}\x1b[0m\x1b[K\r\n", name));
            } else {
                let (prefix, branch) = compute_tree_prefix(&derived.filtered, *filtered_idx);
                output.push_str(&format!(
                    "\x1b[90m{}{}\x1b[0m 📁 \x1b[1;37m{}\x1b[0m\x1b[K\r\n",
                    prefix, branch, name
                ));
            }
        }
    }

    // Render visible items
    for (idx, item) in derived
        .filtered
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(list_height)
    {
        let (folder_idx, cmd_idx) = match item {
            PickerItem::Task { task_id, .. } => (
                derived
                    .folder_indices
                    .get(task_id)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]),
                derived
                    .cmd_indices
                    .get(task_id)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]),
            ),
            PickerItem::Folder { path, .. } => {
                // Find folder_indices from any task in this folder
                let folder_idx = derived
                    .filtered
                    .iter()
                    .filter_map(|i| {
                        if let PickerItem::Task { task_id, .. } = i {
                            let task = state.get_task(task_id)?;
                            let task_folder = folder_key(&task.config_path, root);
                            if task_folder == *path
                                || task_folder.starts_with(&format!("{}/", path))
                                || path.is_empty()
                            {
                                derived.folder_indices.get(task_id).map(|v| v.as_slice())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .next()
                    .unwrap_or(&[]);
                (folder_idx, &[][..])
            }
        };
        output.push_str(&render_item(
            item,
            idx == derived.selected_index,
            state,
            &derived.filtered,
            idx,
            folder_idx,
            cmd_idx,
            root,
        ));
    }

    // Status line
    output.push_str("\x1b[K\r\n");
    let task_count = derived
        .filtered
        .iter()
        .filter(|i| matches!(i, PickerItem::Task { .. }))
        .count();
    let current_task_num = derived
        .filtered
        .iter()
        .take(derived.selected_index + 1)
        .filter(|i| matches!(i, PickerItem::Task { .. }))
        .count();

    match state.mode {
        Mode::Select => output.push_str(&format!(
            "\x1b[90m  {}/{} │ ↑↓ navigate │ tab edit │ enter run │ esc cancel\x1b[0m\x1b[K",
            current_task_num, task_count
        )),
        Mode::Edit => output.push_str(
            "\x1b[90m  edit mode │ ↑↓ back to select │ tab expand │ enter run │ esc cancel\x1b[0m\x1b[K"
        ),
        Mode::Expanded => output.push_str(
            "\x1b[90m  expanded │ ↑↓ back to select │ tab back │ enter run │ esc cancel\x1b[0m\x1b[K"
        ),
    }

    output.push_str("\x1b[J");
    output
}

/// Render input with cursor highlight
fn render_input_cursor(value: &str, cursor: usize) -> (String, char, String) {
    if cursor < value.len() {
        let (before, rest) = value.split_at(cursor);
        let mut chars = rest.chars();
        let current = chars.next().unwrap_or(' ');
        (before.to_string(), current, chars.as_str().to_string())
    } else {
        (value.to_string(), ' ', String::new())
    }
}

/// Find sticky ancestors (folders that should stay visible above scroll)
fn find_sticky_ancestors(
    filtered: &[PickerItem],
    scroll_offset: usize,
) -> Vec<(usize, &PickerItem)> {
    if scroll_offset == 0 || filtered.is_empty() {
        return vec![];
    }
    let first_depth = item_depth(&filtered[scroll_offset]);
    if first_depth == 0 {
        return vec![];
    }

    (0..first_depth)
        .filter_map(|target_depth| {
            (0..scroll_offset).rev().find_map(|i| {
                if let PickerItem::Folder { depth, .. } = &filtered[i] {
                    if *depth == target_depth {
                        return Some((i, &filtered[i]));
                    }
                }
                None
            })
        })
        .collect()
}

/// Compute tree prefix for an item
fn compute_tree_prefix(filtered: &[PickerItem], idx: usize) -> (String, &'static str) {
    fn is_last_at_depth(filtered: &[PickerItem], idx: usize) -> bool {
        let depth = item_depth(&filtered[idx]);
        filtered
            .iter()
            .skip(idx + 1)
            .all(|item| item_depth(item) != depth || item_depth(item) < depth)
    }

    let depth = item_depth(&filtered[idx]);
    let mut prefix = String::from("  ");

    for d in 1..depth {
        let parent_is_last = (0..idx)
            .rev()
            .find(|&j| item_depth(&filtered[j]) == d)
            .map(|j| is_last_at_depth(filtered, j))
            .unwrap_or(true);
        prefix.push_str(if parent_is_last { "   " } else { "│  " });
    }

    (
        prefix,
        if is_last_at_depth(filtered, idx) {
            "└─"
        } else {
            "├─"
        },
    )
}

/// Render a single picker item
#[allow(clippy::too_many_arguments)]
fn render_item(
    item: &PickerItem,
    is_selected: bool,
    state: &AppState,
    filtered: &[PickerItem],
    idx: usize,
    folder_indices: &[u32],
    cmd_indices: &[u32],
    _root: &Path,
) -> String {
    let (prefix, branch) = compute_tree_prefix(filtered, idx);

    match item {
        PickerItem::Folder {
            name,
            path,
            is_root,
            ..
        } => {
            let highlighted_name = render_folder_highlighted(name, path, folder_indices);
            if *is_root {
                format!("  📁 {}\x1b[K\r\n", highlighted_name)
            } else {
                format!(
                    "\x1b[90m{}{}\x1b[0m 📁 {}\x1b[K\r\n",
                    prefix, branch, highlighted_name
                )
            }
        }
        PickerItem::Task { task_id, .. } => {
            let Some(task) = state.get_task(task_id) else {
                return String::new();
            };
            let is_editing = is_selected && matches!(state.mode, Mode::Edit | Mode::Expanded);
            let is_dimmed = matches!(state.mode, Mode::Edit | Mode::Expanded) && !is_selected;
            let marker = if is_selected {
                "\x1b[36m❯\x1b[0m"
            } else {
                " "
            };

            let cmd = if is_editing {
                let (b, c, a) = render_input_cursor(&state.edit_buffer, state.edit_cursor);
                format!("{}\x1b[7m{}\x1b[0m{}", b, c, a)
            } else if is_dimmed {
                format!("\x1b[90m{}\x1b[0m", task.task.command)
            } else {
                render_command_highlighted(&task.task.command, cmd_indices)
            };

            let branch_color = if is_selected { "36" } else { "90" };
            if is_dimmed {
                format!(
                    "\x1b[90m{}{}\x1b[0m {} \x1b[90m{}\x1b[0m  {}\x1b[K\r\n",
                    prefix,
                    branch,
                    marker,
                    task.runner.icon(),
                    cmd
                )
            } else {
                format!(
                    "\x1b[{}m{}{}\x1b[0m {} {}  {}\x1b[K\r\n",
                    branch_color,
                    prefix,
                    branch,
                    marker,
                    task.runner.icon(),
                    cmd
                )
            }
        }
    }
}

/// Render folder name with match highlighting (underline matched chars)
fn render_folder_highlighted(name: &str, path: &str, folder_indices: &[u32]) -> String {
    if folder_indices.is_empty() {
        return format!("\x1b[1;37m{}\x1b[0m", name);
    }

    // folder_indices refer to positions in the full path
    // We need to highlight the portion that falls within the name (last segment)
    let name_start = if path.is_empty() || path == name {
        0
    } else {
        // The name is the last part of the path after the final /
        path.len() - name.len()
    };

    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        let path_idx = (name_start + i) as u32;
        let is_match = folder_indices.contains(&path_idx);
        if is_match {
            result.push_str(&format!("\x1b[1;37;4m{}\x1b[0m", c)); // Bold white + underline
        } else {
            result.push_str(&format!("\x1b[1;37m{}\x1b[0m", c)); // Bold white
        }
    }
    result
}

/// Render command with match highlighting (underline matched chars)
fn render_command_highlighted(command: &str, match_indices: &[u32]) -> String {
    // Parse command structure: "runner [run/task] args..."
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        return command.to_string();
    }

    let mut result = String::new();
    let mut char_idx = 0u32;

    for (part_idx, part) in parts.iter().enumerate() {
        // Add space between parts (except first)
        if part_idx > 0 {
            result.push(' ');
            char_idx += 1;
        }

        // Determine base color for this part
        let base_color = if part_idx == 0 {
            "36" // Cyan for runner (npm, cargo, make, etc.)
        } else if part_idx == 1 && (*part == "run" || *part == "task") {
            "90" // Gray for "run"/"task"
        } else {
            "37" // White for task name/args
        };

        // Render each character with highlight if matched
        for c in part.chars() {
            let is_match = match_indices.contains(&char_idx);
            if is_match {
                // Underline + bold for matches
                result.push_str(&format!("\x1b[{};1;4m{}\x1b[0m", base_color, c));
            } else {
                result.push_str(&format!("\x1b[{}m{}\x1b[0m", base_color, c));
            }
            char_idx += 1;
        }
    }

    result
}

/// Filter a single runner's tasks by query, returning None if no tasks match
fn filter_runner_by_query(
    runner: &TaskRunner,
    pattern: Option<&Pattern>,
    matcher: &mut Matcher,
    root: &Path,
) -> Option<TaskRunner> {
    let Some(pattern) = pattern else {
        return Some(runner.clone());
    };

    let folder = folder_key(&runner.config_path, root);
    let matching_tasks: Vec<Task> = runner
        .tasks
        .iter()
        .filter(|task| {
            let search_text = format!("{} {}", folder, task.command);
            let mut buf = Vec::new();
            let haystack = nucleo_matcher::Utf32Str::new(&search_text, &mut buf);
            pattern.score(haystack, matcher).is_some()
        })
        .cloned()
        .collect();

    if matching_tasks.is_empty() {
        None
    } else {
        Some(TaskRunner {
            config_path: runner.config_path.clone(),
            runner_type: runner.runner_type,
            tasks: matching_tasks,
        })
    }
}

/// Filter all runners by query
fn filter_runners_by_query(
    runners: Vec<TaskRunner>,
    query: Option<&str>,
    root: &Path,
) -> Vec<TaskRunner> {
    let Some(query) = query else {
        return runners;
    };

    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);

    runners
        .into_iter()
        .filter_map(|runner| filter_runner_by_query(&runner, Some(&pattern), &mut matcher, root))
        .collect()
}

fn main() {
    let cli = Cli::parse();

    let root = cli
        .path
        .unwrap_or_else(|| env::current_dir().expect("Failed to get current directory"));

    let root = root.canonicalize().unwrap_or_else(|_| root.clone());

    let options = ScanOptions {
        no_ignore: cli.no_ignore,
        ..Default::default()
    };

    // JSON array output mode - collect all results then output
    if cli.json {
        let runners = scan_with_options(&root, options.clone()).unwrap_or_default();
        let runners = filter_runners_by_query(runners, cli.query.as_deref(), &root);
        println!(
            "{}",
            serde_json::to_string_pretty(&runners).unwrap_or_else(|_| "[]".into())
        );
        return;
    }

    // NDJSON streaming output mode - output each result as found
    if cli.json_stream {
        let (tx, rx) = mpsc::channel();
        let _scanner_handle = scan_streaming(root.clone(), options, tx);

        let mut stdout = stdout().lock();
        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let pattern = cli
            .query
            .as_ref()
            .map(|q| Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart));

        for runner in rx {
            let filtered = filter_runner_by_query(&runner, pattern.as_ref(), &mut matcher, &root);
            if let Some(filtered) = filtered {
                writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&filtered).unwrap_or_default()
                )
                .ok();
            }
        }
        return;
    }

    // Interactive mode - use streaming scan
    let (tx, rx) = mpsc::channel();
    let _scanner_handle = scan_streaming(root.clone(), options, tx);

    match run_picker(&rx, &root) {
        Some(result) => {
            run_task(&result.task, &result.command, &root);
        }
        None => {
            println!();
            println!("  {} Cancelled", style("✗").dim());
        }
    }
}

/// Run the picker with streaming input
fn run_picker(rx: &mpsc::Receiver<TaskRunner>, root: &Path) -> Option<PickerResult> {
    // Setup terminal
    terminal::enable_raw_mode().ok()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, Hide).ok()?;

    let result = run_picker_loop(rx, root, &mut stdout);

    // Restore terminal
    execute!(stdout, Show, LeaveAlternateScreen).ok();
    terminal::disable_raw_mode().ok();

    result
}

/// Main picker loop
fn run_picker_loop(
    rx: &mpsc::Receiver<TaskRunner>,
    root: &Path,
    stdout: &mut io::Stdout,
) -> Option<PickerResult> {
    let mut state = AppState::default();
    let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);

    loop {
        // Collect and process events
        for event in collect_events(rx, state.scanning_done, root) {
            state = match event {
                AppEvent::TasksReceived { tasks, folders } => {
                    let mut by_id = state.tasks_by_id.clone();
                    let mut ids = state.task_ids.clone();
                    for task in tasks {
                        let id = TaskId::from_task(&task);
                        if let std::collections::hash_map::Entry::Vacant(e) =
                            by_id.entry(id.clone())
                        {
                            ids.push(id);
                            e.insert(task);
                        }
                    }
                    let mut folder_ids = state.folder_ids.clone();
                    for f in folders {
                        if !folder_ids.contains(&f) {
                            folder_ids.push(f);
                        }
                    }
                    AppState {
                        tasks_by_id: by_id,
                        task_ids: ids,
                        folder_ids,
                        ..state
                    }
                }
                AppEvent::ScanComplete => AppState {
                    scanning_done: true,
                    ..state
                },
                AppEvent::Key(key) => {
                    let items = derive_items(&state, root);
                    let matching =
                        compute_matching_tasks(&items, &state, &state.query, root, &mut matcher);
                    let derived = derive_all(&state, root, &matching);
                    match handle_key(state.clone(), key, &derived) {
                        UpdateResult::Continue(s) => s,
                        UpdateResult::Exit(result) => return result,
                    }
                }
            };
        }

        // Derive and render
        let items = derive_items(&state, root);
        let matching = compute_matching_tasks(&items, &state, &state.query, root, &mut matcher);
        let derived = derive_all(&state, root, &matching);

        let (_, height) = terminal::size().unwrap_or((80, 24));
        let list_height = (height as usize).saturating_sub(
            8 + find_sticky_ancestors(&derived.filtered, state.scroll_offset).len(),
        );
        state.scroll_offset =
            derive_scroll(derived.selected_index, state.scroll_offset, list_height);

        execute!(stdout, MoveTo(0, 0)).ok();
        write!(
            stdout,
            "{}",
            render(&state, &derived, root, height as usize)
        )
        .ok();
        stdout.flush().ok();
    }
}

/// Collect events from channel and keyboard
fn collect_events(
    rx: &mpsc::Receiver<TaskRunner>,
    scanning_done: bool,
    root: &Path,
) -> Vec<AppEvent> {
    let mut events = Vec::new();

    // Receive tasks from scanner
    loop {
        match rx.try_recv() {
            Ok(runner) => {
                let folder = folder_key(&runner.config_path, root);

                let tasks: Vec<DisplayTask> = runner
                    .tasks
                    .into_iter()
                    .map(|task| DisplayTask {
                        runner: runner.runner_type,
                        config_path: runner.config_path.clone(),
                        task,
                    })
                    .collect();

                events.push(AppEvent::TasksReceived {
                    tasks,
                    folders: vec![folder],
                });
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                if !scanning_done {
                    events.push(AppEvent::ScanComplete);
                }
                break;
            }
        }
    }

    // Check for keyboard input
    if event::poll(Duration::from_millis(50)).unwrap_or(false) {
        if let Ok(CrosstermEvent::Key(key)) = event::read() {
            events.push(AppEvent::Key(key));
        }
    }

    events
}

/// Run a task
fn run_task(task: &DisplayTask, command: &str, root: &Path) {
    let work_dir = task.config_path.parent().unwrap_or(root);
    let sep = style("─".repeat(60)).dim();

    println!(
        "\n  {} {} {}",
        task.runner.icon(),
        style("Running").green().bold(),
        style(command).white().bold()
    );
    if work_dir != root {
        println!(
            "  {} {}",
            style("in").dim(),
            style(work_dir.strip_prefix(root).unwrap_or(work_dir).display()).dim()
        );
    }
    println!("\n{}\n", sep);

    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        eprintln!("{} Empty command", style("✗").red());
        return;
    }

    let status = Command::new(parts[0])
        .args(&parts[1..])
        .current_dir(work_dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    println!("\n{}", sep);
    match status {
        Ok(s) if s.success() => println!(
            "\n  {} {}\n",
            style("✓").green().bold(),
            style("Task completed successfully").green()
        ),
        Ok(s) => {
            println!(
                "\n  {} {} {}\n",
                style("✗").red().bold(),
                style("Task failed with exit code").red(),
                style(s.code().unwrap_or(-1)).red().bold()
            );
            std::process::exit(s.code().unwrap_or(1));
        }
        Err(e) => {
            println!(
                "\n  {} {} {}\n",
                style("✗").red().bold(),
                style("Failed to execute:").red(),
                style(e).red()
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that the first render matches the expected output
    #[test]
    fn test_first_render_matches_expected() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let root = PathBuf::from(manifest_dir);

        // Scan the project root
        let runners = task_runner_detector::scan_with_options(&root, ScanOptions::default())
            .unwrap_or_default();

        // Build state from runners
        let mut state = AppState::default();
        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);

        for runner in &runners {
            let folder = folder_key(&runner.config_path, &root);
            if !state.folder_ids.contains(&folder) {
                state.folder_ids.push(folder.clone());
            }

            for task in &runner.tasks {
                let display_task = DisplayTask {
                    runner: runner.runner_type,
                    config_path: runner.config_path.clone(),
                    task: task.clone(),
                };
                let id = TaskId::from_task(&display_task);
                if !state.tasks_by_id.contains_key(&id) {
                    state.task_ids.push(id.clone());
                    state.tasks_by_id.insert(id, display_task);
                }
            }
        }
        state.scanning_done = true;

        // Select the first task
        let items = derive_items(&state, &root);
        let matching = compute_matching_tasks(&items, &state, "", &root, &mut matcher);

        // Find first task and select it
        for mt in &matching {
            state.selected_task = Some(mt.task_id.clone());
            break;
        }

        let derived = derive_all(&state, &root, &matching);

        // Render with enough terminal height to show all items
        let output = render(&state, &derived, &root, 50);

        // Read expected output and compare
        let expected_path = root.join("fixtures/first_render.txt");
        let expected = std::fs::read_to_string(&expected_path)
            .expect("Failed to read fixtures/first_render.txt");

        assert_eq!(
            output, expected,
            "Render output doesn't match expected fixture"
        );
    }
}
