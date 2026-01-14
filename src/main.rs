//! Task CLI - Discover and run tasks from various config files
//!
//! Usage:
//!   task              # Interactive wizard (scan cwd, select, run)
//!   task <path>       # Interactive wizard for specific directory
//!   task --json       # JSON output for cwd
//!   task --json <path>  # JSON output for specific directory

use std::env;
use std::io::{self, stdout, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use clap::Parser;
use console::style;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Matcher,
};
use tui_input::Input;

use task_runner_detector::{scan_with_options, RunnerType, ScanOptions, Task, TaskRunner};

#[derive(Parser)]
#[command(name = "task")]
#[command(about = "Discover and run tasks from various config files")]
#[command(version)]
struct Cli {
    /// Output results as JSON instead of interactive mode
    #[arg(long)]
    json: bool,

    /// Don't respect .gitignore and scan all files
    #[arg(long)]
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

/// An item in the picker - either a folder header or a task
#[derive(Clone)]
enum PickerItem {
    Folder {
        name: String,
        path: String, // Full path for filtering (e.g., "apps/mobile")
        is_root: bool,
        original_depth: usize, // Used for computing dynamic tree structure
    },
    Task {
        idx: usize,
        depth: usize,
    },
}

fn main() {
    let cli = Cli::parse();

    let root = cli
        .path
        .unwrap_or_else(|| env::current_dir().expect("Failed to get current directory"));

    let root = root.canonicalize().unwrap_or_else(|_| root.clone());

    // Scan for tasks
    let options = ScanOptions {
        no_ignore: cli.no_ignore,
        ..Default::default()
    };
    let runners = match scan_with_options(&root, options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} Error scanning directory: {}", style("✗").red(), e);
            std::process::exit(1);
        }
    };

    if runners.is_empty() {
        if cli.json {
            println!("[]");
        } else {
            eprintln!(
                "{} No task runners found in {}",
                style("✗").yellow(),
                root.display()
            );
        }
        return;
    }

    // JSON output mode
    if cli.json {
        let json = serde_json::to_string_pretty(&runners).expect("Failed to serialize to JSON");
        println!("{}", json);
        return;
    }

    // Interactive mode
    run_interactive(runners, root);
}

/// Group key: folder path for grouping
fn folder_key(task: &DisplayTask, root: &PathBuf) -> String {
    let relative = task
        .config_path
        .strip_prefix(root)
        .unwrap_or(&task.config_path);

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

fn run_interactive(runners: Vec<TaskRunner>, root: PathBuf) {
    // Collect all tasks
    let mut all_tasks: Vec<DisplayTask> = Vec::new();
    for runner in runners {
        for task in runner.tasks {
            all_tasks.push(DisplayTask {
                runner: runner.runner_type,
                config_path: runner.config_path.clone(),
                task,
            });
        }
    }

    if all_tasks.is_empty() {
        eprintln!("{} No tasks found", style("✗").yellow());
        return;
    }

    // Sort tasks: by folder depth, then folder name, then runner, then task name
    all_tasks.sort_by(|a, b| {
        let a_folder = folder_key(a, &root);
        let b_folder = folder_key(b, &root);
        let a_depth = if a_folder == "." { 0 } else { a_folder.matches('/').count() + 1 };
        let b_depth = if b_folder == "." { 0 } else { b_folder.matches('/').count() + 1 };

        a_depth
            .cmp(&b_depth)
            .then_with(|| a_folder.cmp(&b_folder))
            .then_with(|| a.runner.display_name().cmp(b.runner.display_name()))
            .then_with(|| a.task.name.cmp(&b.task.name))
    });

    // Build tree structure
    // First, collect all unique folder paths and their tasks
    use std::collections::BTreeMap;

    let mut folder_tasks: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, task) in all_tasks.iter().enumerate() {
        let folder = folder_key(task, &root);
        folder_tasks.entry(folder).or_default().push(idx);
    }

    // Build a tree of folders
    #[derive(Default)]
    struct FolderNode {
        tasks: Vec<usize>,
        children: BTreeMap<String, FolderNode>,
    }

    let mut tree = FolderNode::default();

    for (path, tasks) in &folder_tasks {
        if path == "." {
            tree.tasks = tasks.clone();
        } else {
            let parts: Vec<&str> = path.split('/').collect();
            let mut current = &mut tree;
            for part in parts {
                current = current.children.entry(part.to_string()).or_default();
            }
            current.tasks = tasks.clone();
        }
    }

    // Flatten tree into picker items with proper indentation
    let mut items: Vec<PickerItem> = Vec::new();

    fn flatten_tree(
        node: &FolderNode,
        name: &str,
        path: &str, // Full path like "apps/mobile"
        depth: usize,
        items: &mut Vec<PickerItem>,
        is_root: bool,
    ) {
        // Always show folder header (including root)
        items.push(PickerItem::Folder {
            name: name.to_string(),
            path: path.to_string(),
            is_root,
            original_depth: depth,
        });

        // Children are always one level deeper
        let child_depth = depth + 1;

        // Add tasks first
        for &task_idx in node.tasks.iter() {
            items.push(PickerItem::Task {
                idx: task_idx,
                depth: child_depth,
            });
        }

        // Then add child folders
        let children: Vec<_> = node.children.iter().collect();
        for (child_name, child_node) in children.iter() {
            // Build child path
            let child_path = if path.is_empty() {
                child_name.to_string()
            } else {
                format!("{}/{}", path, child_name)
            };
            flatten_tree(
                child_node,
                child_name,
                &child_path,
                child_depth,
                items,
                false, // Not root
            );
        }
    }

    // Get root folder name for display
    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    // Start with root folder (empty path since root is not part of folder_key paths)
    flatten_tree(&tree, &root_name, "", 0, &mut items, true);

    // Run the custom picker
    match run_picker(&items, &all_tasks, &root) {
        Some(result) => {
            let task = &all_tasks[result.task_idx];
            run_task(task, &result.command, &root);
        }
        None => {
            println!();
            println!("  {} Cancelled", style("✗").dim());
        }
    }
}

/// Result from the picker: task index and the (potentially modified) command to run
struct PickerResult {
    task_idx: usize,
    command: String,
}

fn run_picker(items: &[PickerItem], all_tasks: &[DisplayTask], root: &PathBuf) -> Option<PickerResult> {
    // Enable virtual terminal processing on Windows for ANSI support
    #[cfg(windows)]
    let _ = crossterm::ansi_support::supports_ansi();

    // Setup terminal
    terminal::enable_raw_mode().ok()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, Hide).ok()?;

    let result = run_picker_inner(items, all_tasks, root, &mut stdout);

    // Restore terminal
    execute!(stdout, Show, LeaveAlternateScreen).ok();
    terminal::disable_raw_mode().ok();

    result
}

/// Whether we're in select mode (filtering tasks) or edit mode (editing command)
#[derive(PartialEq)]
enum PickerMode {
    Select,
    Edit,
}

fn run_picker_inner(
    items: &[PickerItem],
    all_tasks: &[DisplayTask],
    root: &PathBuf,
    stdout: &mut io::Stdout,
) -> Option<PickerResult> {
    let mut query_input = Input::default();
    let mut selected: usize = 0;
    let mut scroll_offset: usize = 0;

    // Edit mode state
    let mut mode = PickerMode::Select;
    let mut edit_input = Input::default();

    let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);

    loop {
        // Filter items based on query using fuzzy matching
        let query = query_input.value();
        let filtered: Vec<(usize, &PickerItem)> = if query.is_empty() {
            items.iter().enumerate().collect()
        } else {
            let pattern = Pattern::parse(&query, CaseMatching::Ignore, Normalization::Smart);

            // First, find which tasks match with fuzzy matching
            let matching_tasks: Vec<usize> = items
                .iter()
                .filter_map(|item| {
                    if let PickerItem::Task { idx, .. } = item {
                        let task = &all_tasks[*idx];
                        let search_text = format!(
                            "{} {} {}",
                            task.task.name,
                            task.runner.display_name(),
                            folder_key(task, root)
                        );
                        let mut buf = Vec::new();
                        let haystack = nucleo_matcher::Utf32Str::new(&search_text, &mut buf);
                        if pattern.score(haystack, &mut matcher).is_some() {
                            return Some(*idx);
                        }
                    }
                    None
                })
                .collect();

            // Find which folder paths have matching tasks (including all parent paths)
            let mut matching_folder_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
            // Always include root (empty path)
            matching_folder_paths.insert(String::new());

            for idx in &matching_tasks {
                let folder = folder_key(&all_tasks[*idx], root);
                // Add the folder and all its parents
                if folder != "." {
                    let parts: Vec<&str> = folder.split('/').collect();
                    let mut path = String::new();
                    for (i, part) in parts.iter().enumerate() {
                        if i > 0 {
                            path.push('/');
                        }
                        path.push_str(part);
                        matching_folder_paths.insert(path.clone());
                    }
                }
            }

            // Include folders whose path matches, and matching tasks
            items
                .iter()
                .enumerate()
                .filter(|(_, item)| match item {
                    PickerItem::Folder { path, is_root, .. } => {
                        // Always show root, or show if path is in matching set
                        *is_root || matching_folder_paths.contains(path)
                    }
                    PickerItem::Task { idx, .. } => matching_tasks.contains(idx),
                })
                .collect()
        };

        // Clamp selection
        if filtered.is_empty() {
            selected = 0;
        } else if selected >= filtered.len() {
            selected = filtered.len() - 1;
        }

        // Skip folders in selection (only tasks are selectable)
        while selected < filtered.len() {
            if let PickerItem::Folder { .. } = filtered[selected].1 {
                if selected + 1 < filtered.len() {
                    selected += 1;
                } else if selected > 0 {
                    selected -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // Get terminal size
        let (_width, height) = terminal::size().unwrap_or((80, 24));
        let base_list_height = (height as usize).saturating_sub(8);

        // Find sticky ancestor folders for current scroll position
        // These are folders that are ancestors of visible items but scrolled out of view
        // Returns (filtered_index, &PickerItem) so we can compute tree structure
        fn find_sticky_ancestors<'a>(
            filtered: &'a [(usize, &'a PickerItem)],
            scroll_offset: usize,
        ) -> Vec<(usize, &'a PickerItem)> {
            if scroll_offset == 0 || filtered.is_empty() {
                return vec![];
            }

            // Get the first visible item
            let first_visible = &filtered[scroll_offset].1;
            let first_depth = match first_visible {
                PickerItem::Folder { original_depth, .. } => *original_depth,
                PickerItem::Task { depth, .. } => *depth,
            };

            if first_depth == 0 {
                return vec![];
            }

            // Find ancestor folders at each depth level that are before scroll_offset
            let mut ancestors: Vec<(usize, &PickerItem)> = Vec::new();
            for target_depth in 0..first_depth {
                // Walk backwards from scroll_offset to find the folder at this depth
                for i in (0..scroll_offset).rev() {
                    let item = filtered[i].1;
                    if let PickerItem::Folder { original_depth, .. } = item {
                        if *original_depth == target_depth {
                            ancestors.push((i, item));
                            break;
                        }
                    }
                }
            }

            ancestors
        }

        let sticky_ancestors = find_sticky_ancestors(&filtered, scroll_offset);
        let sticky_count = sticky_ancestors.len();
        let list_height = base_list_height.saturating_sub(sticky_count);

        // Adjust scroll offset
        if selected < scroll_offset {
            scroll_offset = selected;
        } else if selected >= scroll_offset + list_height {
            scroll_offset = selected - list_height + 1;
        }

        // Recompute sticky ancestors after scroll adjustment
        let sticky_ancestors = find_sticky_ancestors(&filtered, scroll_offset);
        let sticky_count = sticky_ancestors.len();
        let list_height = base_list_height.saturating_sub(sticky_count);

        // Helper to get depth of an item
        fn item_depth(item: &PickerItem) -> usize {
            match item {
                PickerItem::Folder { original_depth, .. } => *original_depth,
                PickerItem::Task { depth, .. } => *depth,
            }
        }

        // Check if item at index i is the last at its depth level in filtered list
        fn is_last_in_filtered(filtered: &[(usize, &PickerItem)], i: usize) -> bool {
            let current_depth = item_depth(filtered[i].1);
            // Look at remaining items to see if there's a sibling
            for j in (i + 1)..filtered.len() {
                let next_depth = item_depth(filtered[j].1);
                if next_depth < current_depth {
                    // We've gone up in the tree, so current was last at its level
                    return true;
                }
                if next_depth == current_depth {
                    // There's a sibling at the same level
                    return false;
                }
                // next_depth > current_depth means it's a child, keep looking
            }
            // Reached end of list, so it's last
            true
        }

        // Compute dynamic parent_lasts for filtered view
        fn compute_filtered_parent_lasts(filtered: &[(usize, &PickerItem)], i: usize) -> Vec<bool> {
            let current_depth = item_depth(filtered[i].1);
            if current_depth <= 1 {
                return vec![];
            }

            let mut parent_lasts = Vec::new();

            // Walk backwards to find ancestors at each depth level and check if they're last
            for d in 1..current_depth {
                // Find the ancestor at depth d by walking backwards
                for j in (0..i).rev() {
                    let ancestor_depth = item_depth(filtered[j].1);
                    if ancestor_depth == d {
                        parent_lasts.push(is_last_in_filtered(filtered, j));
                        break;
                    }
                }
            }

            parent_lasts
        }

        // Build output buffer - use \x1b[K to clear to end of line instead of full screen clear
        let mut output = String::new();

        // Move to top (no full screen clear to avoid flicker)
        execute!(stdout, MoveTo(0, 0)).ok();

        // Header - \x1b[K clears from cursor to end of line
        output.push_str("\x1b[36m  Task Runner Detector\x1b[0m\x1b[K\r\n");
        output.push_str(&format!("\x1b[90m  {} tasks found\x1b[0m\x1b[K\r\n", all_tasks.len()));
        output.push_str("\x1b[K\r\n");

        // Input line - show filter query (cursor only in select mode)
        // In edit mode, the cursor is shown inline on the selected task
        fn render_input_with_cursor(input: &Input) -> (String, char, String) {
            let value = input.value();
            let cursor = input.cursor();
            if cursor < value.len() {
                let (before, rest) = value.split_at(cursor);
                let mut chars = rest.chars();
                let current = chars.next().unwrap_or(' ');
                (before.to_string(), current, chars.as_str().to_string())
            } else {
                (value.to_string(), ' ', String::new())
            }
        }

        if mode == PickerMode::Select {
            let (before, at, after) = render_input_with_cursor(&query_input);
            output.push_str(&format!(
                "\x1b[36m❯ \x1b[0m{}\x1b[7m{}\x1b[0m{}\x1b[K\r\n",
                before, at, after
            ));
        } else {
            // Edit mode - show query without cursor (cursor is on the task)
            output.push_str(&format!(
                "\x1b[90m❯ {}\x1b[0m\x1b[K\r\n",
                query_input.value()
            ));
        }
        output.push_str("\x1b[K\r\n");

        // Render sticky ancestor headers (using same tree logic as normal rendering)
        for (filtered_idx, ancestor) in &sticky_ancestors {
            if let PickerItem::Folder { name, is_root, .. } = ancestor {
                if *is_root {
                    output.push_str(&format!("  📁 \x1b[1;37m{}\x1b[0m\x1b[K\r\n", name));
                } else {
                    // Use the same tree structure computation as normal folders
                    let is_last = is_last_in_filtered(&filtered, *filtered_idx);
                    let parent_lasts = compute_filtered_parent_lasts(&filtered, *filtered_idx);

                    let mut prefix = String::from("  ");
                    for &parent_is_last in parent_lasts.iter() {
                        if parent_is_last {
                            prefix.push_str("   ");
                        } else {
                            prefix.push_str("│  ");
                        }
                    }

                    let branch = if is_last { "└─" } else { "├─" };
                    output.push_str(&format!("\x1b[90m{}{}\x1b[0m 📁 \x1b[1;37m{}\x1b[0m\x1b[K\r\n", prefix, branch, name));
                }
            }
        }

        // List items
        let visible_items: Vec<_> = filtered
            .iter()
            .skip(scroll_offset)
            .take(list_height)
            .collect();

        for (i, (_, item)) in visible_items.iter().enumerate() {
            let is_selected = scroll_offset + i == selected;
            let filtered_idx = scroll_offset + i;

            // Compute is_last dynamically based on filtered results
            let is_last_dynamic = is_last_in_filtered(&filtered, filtered_idx);
            let parent_lasts_dynamic = compute_filtered_parent_lasts(&filtered, filtered_idx);

            match item {
                PickerItem::Folder { name, is_root, .. } => {
                    if *is_root {
                        // Root folder - no branch, just the name
                        output.push_str(&format!("  📁 \x1b[1;37m{}\x1b[0m\x1b[K\r\n", name));
                    } else {
                        // Build tree prefix from parent_lasts
                        let mut prefix = String::from("  ");
                        for &parent_is_last in parent_lasts_dynamic.iter() {
                            if parent_is_last {
                                prefix.push_str("   ");
                            } else {
                                prefix.push_str("│  ");
                            }
                        }

                        // Add the branch for this folder
                        let branch = if is_last_dynamic { "└─" } else { "├─" };

                        // Nested folder - dim branch, bold white name
                        output.push_str(&format!("\x1b[90m{}{}\x1b[0m 📁 \x1b[1;37m{}\x1b[0m\x1b[K\r\n", prefix, branch, name));
                    }
                }
                PickerItem::Task { idx, depth, .. } => {
                    let task = &all_tasks[*idx];
                    let icon = task.runner.icon();

                    // In edit mode, non-selected tasks are dimmed
                    let is_editing_this = is_selected && mode == PickerMode::Edit;
                    let is_dimmed = mode == PickerMode::Edit && !is_selected;

                    let runner_color = match task.runner.color_code() {
                        1 => "31", // Red
                        2 => "32", // Green
                        3 => "33", // Yellow
                        4 => "34", // Blue
                        5 => "35", // Magenta
                        6 => "36", // Cyan
                        _ => "37", // White
                    };

                    // Build tree prefix from dynamic parent_lasts
                    let mut prefix = String::from("  ");
                    for &parent_is_last in parent_lasts_dynamic.iter() {
                        if parent_is_last {
                            prefix.push_str("   ");
                        } else {
                            prefix.push_str("│  ");
                        }
                    }

                    // Tree branch characters - use dynamic is_last
                    let branch = if is_last_dynamic { "└─" } else { "├─" };
                    let branch_color = if is_dimmed { "90" } else if is_selected { "36" } else { "90" };

                    let selection_marker = if is_selected { "\x1b[36m❯\x1b[0m" } else { " " };

                    // Render the command - with cursor if editing, colored otherwise
                    let command_display = if is_editing_this {
                        // Show with inline cursor
                        let (before, at, after) = render_input_with_cursor(&edit_input);
                        format!("{}\x1b[7m{}\x1b[0m{}", before, at, after)
                    } else if is_dimmed {
                        // Fully dimmed
                        format!("\x1b[90m{}\x1b[0m", task.task.command)
                    } else {
                        // Normal colored display: "npm run build" -> colored npm, dim run, bold build
                        let cmd_parts: Vec<&str> = task.task.command.split_whitespace().collect();
                        let cmd_runner = cmd_parts.first().unwrap_or(&"");

                        if cmd_parts.len() >= 3 && (cmd_parts[1] == "run" || cmd_parts[1] == "task") {
                            let subcommand = cmd_parts[1];
                            let task_name = cmd_parts[2..].join(" ");
                            format!(
                                "\x1b[{}m{}\x1b[0m \x1b[90m{}\x1b[0m \x1b[1;37m{}\x1b[0m",
                                runner_color, cmd_runner, subcommand, task_name
                            )
                        } else if cmd_parts.len() >= 2 {
                            let task_name = cmd_parts[1..].join(" ");
                            format!(
                                "\x1b[{}m{}\x1b[0m \x1b[1;37m{}\x1b[0m",
                                runner_color, cmd_runner, task_name
                            )
                        } else {
                            format!("\x1b[{}m{}\x1b[0m", runner_color, task.task.command)
                        }
                    };

                    if *depth == 0 {
                        // Root level task, minimal indentation
                        if is_dimmed {
                            output.push_str(&format!(
                                "  {} \x1b[90m{}\x1b[0m  {}\x1b[K\r\n",
                                selection_marker,
                                icon,
                                command_display,
                            ));
                        } else {
                            output.push_str(&format!(
                                "  {}{} {}  {}\x1b[K\r\n",
                                selection_marker,
                                if is_selected { "\x1b[36m" } else { "" },
                                icon,
                                command_display,
                            ));
                        }
                    } else {
                        // Nested task with tree branch
                        if is_dimmed {
                            output.push_str(&format!(
                                "\x1b[90m{}{}\x1b[0m {} \x1b[90m{}\x1b[0m  {}\x1b[K\r\n",
                                prefix,
                                branch,
                                selection_marker,
                                icon,
                                command_display,
                            ));
                        } else {
                            output.push_str(&format!(
                                "\x1b[{}m{}{}\x1b[0m {} {}  {}\x1b[K\r\n",
                                branch_color,
                                prefix,
                                branch,
                                selection_marker,
                                icon,
                                command_display,
                            ));
                        }
                    }
                }
            }
        }

        // Status line - different hints for each mode
        output.push_str("\x1b[K\r\n");
        if mode == PickerMode::Select {
            output.push_str(&format!(
                "\x1b[90m  {}/{} │ ↑↓ navigate │ tab edit │ enter run │ esc cancel\x1b[0m\x1b[K",
                filtered.iter().filter(|(_, i)| matches!(i, PickerItem::Task { .. })).count(),
                all_tasks.len()
            ));
        } else {
            output.push_str(
                "\x1b[90m  edit mode │ ↑↓ back to select │ tab back │ enter run │ esc cancel\x1b[0m\x1b[K"
            );
        }

        // Clear any remaining lines below (in case list got shorter)
        output.push_str("\x1b[J");

        // Write all at once
        write!(stdout, "{}", output).ok();
        stdout.flush().ok();

        // Handle input
        if let Ok(event) = event::read() {
            if let Event::Key(key) = &event {
                match key.code {
                    KeyCode::Esc => return None,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return None;
                    }
                    KeyCode::Enter => {
                        if let Some((_, PickerItem::Task { idx, .. })) = filtered.get(selected) {
                            let command = if mode == PickerMode::Edit {
                                edit_input.value().to_string()
                            } else {
                                all_tasks[*idx].task.command.clone()
                            };
                            return Some(PickerResult { task_idx: *idx, command });
                        }
                    }
                    KeyCode::Tab => {
                        if mode == PickerMode::Select {
                            // Enter edit mode with current task's command
                            if let Some((_, PickerItem::Task { idx, .. })) = filtered.get(selected) {
                                edit_input = all_tasks[*idx].task.command.as_str().into();
                                // Move cursor to end
                                edit_input.handle(tui_input::InputRequest::GoToEnd);
                                mode = PickerMode::Edit;
                            }
                        } else {
                            // Back to select mode
                            mode = PickerMode::Select;
                        }
                    }
                    KeyCode::Up => {
                        if mode == PickerMode::Edit {
                            mode = PickerMode::Select;
                        }
                        if selected > 0 {
                            selected -= 1;
                            while selected > 0 {
                                if let PickerItem::Folder { .. } = filtered[selected].1 {
                                    selected -= 1;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                    KeyCode::Down => {
                        if mode == PickerMode::Edit {
                            mode = PickerMode::Select;
                        }
                        if selected + 1 < filtered.len() {
                            selected += 1;
                            while selected + 1 < filtered.len() {
                                if let PickerItem::Folder { .. } = filtered[selected].1 {
                                    selected += 1;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                    _ => {
                        // Convert key event to tui-input request
                        use tui_input::InputRequest;
                        let request = match key.code {
                            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                Some(InputRequest::GoToStart)
                            }
                            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                Some(InputRequest::GoToEnd)
                            }
                            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                Some(InputRequest::DeletePrevWord)
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                Some(InputRequest::DeleteLine)
                            }
                            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                Some(InputRequest::DeleteTillEnd)
                            }
                            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                Some(InputRequest::DeleteNextChar)
                            }
                            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                Some(InputRequest::GoToPrevChar)
                            }
                            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                Some(InputRequest::GoToNextChar)
                            }
                            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
                                Some(InputRequest::GoToPrevWord)
                            }
                            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
                                Some(InputRequest::GoToNextWord)
                            }
                            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::ALT) => {
                                Some(InputRequest::DeleteNextWord)
                            }
                            KeyCode::Left => Some(InputRequest::GoToPrevChar),
                            KeyCode::Right => Some(InputRequest::GoToNextChar),
                            KeyCode::Home => Some(InputRequest::GoToStart),
                            KeyCode::End => Some(InputRequest::GoToEnd),
                            KeyCode::Backspace => Some(InputRequest::DeletePrevChar),
                            KeyCode::Delete => Some(InputRequest::DeleteNextChar),
                            KeyCode::Char(c) => Some(InputRequest::InsertChar(c)),
                            _ => None,
                        };

                        if let Some(req) = request {
                            if mode == PickerMode::Edit {
                                edit_input.handle(req);
                            } else {
                                let prev_value = query_input.value().to_string();
                                query_input.handle(req);
                                // Reset selection if query changed
                                if query_input.value() != prev_value {
                                    selected = 0;
                                    scroll_offset = 0;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn run_task(task: &DisplayTask, command: &str, root: &PathBuf) {
    println!();
    println!(
        "  {} {} {}",
        task.runner.icon(),
        style("Running").green().bold(),
        style(command).white().bold()
    );

    let work_dir = task.config_path.parent().unwrap_or(root);
    if work_dir != root {
        println!(
            "  {} {}",
            style("in").dim(),
            style(work_dir.strip_prefix(root).unwrap_or(work_dir).display()).dim()
        );
    }
    println!();
    println!("{}", style("─".repeat(60)).dim());
    println!();

    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        eprintln!("{} Empty command", style("✗").red());
        return;
    }

    let (program, args) = (parts[0], &parts[1..]);

    let status = Command::new(program)
        .args(args)
        .current_dir(work_dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    println!();
    println!("{}", style("─".repeat(60)).dim());

    match status {
        Ok(status) => {
            if status.success() {
                println!();
                println!(
                    "  {} {}",
                    style("✓").green().bold(),
                    style("Task completed successfully").green()
                );
                println!();
            } else {
                println!();
                println!(
                    "  {} {} {}",
                    style("✗").red().bold(),
                    style("Task failed with exit code").red(),
                    style(status.code().unwrap_or(-1)).red().bold()
                );
                println!();
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        Err(e) => {
            println!();
            println!(
                "  {} {} {}",
                style("✗").red().bold(),
                style("Failed to execute:").red(),
                style(e).red()
            );
            println!();
            std::process::exit(1);
        }
    }
}
