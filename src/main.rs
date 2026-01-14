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
        Some(task_idx) => {
            let task = &all_tasks[task_idx];
            run_task(task, &root);
        }
        None => {
            println!();
            println!("  {} Cancelled", style("✗").dim());
        }
    }
}

fn run_picker(items: &[PickerItem], all_tasks: &[DisplayTask], root: &PathBuf) -> Option<usize> {
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

fn run_picker_inner(
    items: &[PickerItem],
    all_tasks: &[DisplayTask],
    root: &PathBuf,
    stdout: &mut io::Stdout,
) -> Option<usize> {
    let mut query = String::new();
    let mut selected: usize = 0;
    let mut scroll_offset: usize = 0;

    loop {
        // Filter items based on query
        let filtered: Vec<(usize, &PickerItem)> = if query.is_empty() {
            items.iter().enumerate().collect()
        } else {
            let query_lower = query.to_lowercase();

            // First, find which tasks match
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
                        )
                        .to_lowercase();
                        if search_text.contains(&query_lower) {
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
        let list_height = (height as usize).saturating_sub(8);

        // Adjust scroll offset
        if selected < scroll_offset {
            scroll_offset = selected;
        } else if selected >= scroll_offset + list_height {
            scroll_offset = selected - list_height + 1;
        }

        // Build output buffer - use \x1b[K to clear to end of line instead of full screen clear
        let mut output = String::new();

        // Move to top (no full screen clear to avoid flicker)
        execute!(stdout, MoveTo(0, 0)).ok();

        // Header - \x1b[K clears from cursor to end of line
        output.push_str("\x1b[36m  Task Runner Detector\x1b[0m\x1b[K\r\n");
        output.push_str(&format!("\x1b[90m  {} tasks found\x1b[0m\x1b[K\r\n", all_tasks.len()));
        output.push_str("\x1b[K\r\n");

        // Input line
        output.push_str(&format!("\x1b[36m❯ \x1b[0m{}█\x1b[K\r\n", query));
        output.push_str("\x1b[K\r\n");

        // List items
        let visible_items: Vec<_> = filtered
            .iter()
            .skip(scroll_offset)
            .take(list_height)
            .collect();

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

                    let runner_color = match task.runner.color_code() {
                        1 => "31", // Red
                        2 => "32", // Green
                        3 => "33", // Yellow
                        4 => "34", // Blue
                        5 => "35", // Magenta
                        6 => "36", // Cyan
                        _ => "37", // White
                    };

                    // Split command: "npm run build" -> runner="npm", subcommand="run", task="build"
                    let cmd_parts: Vec<&str> = task.task.command.split_whitespace().collect();
                    let cmd_runner = cmd_parts.first().unwrap_or(&"");

                    let (subcommand, task_name) = if cmd_parts.len() >= 3 && (cmd_parts[1] == "run" || cmd_parts[1] == "task") {
                        (cmd_parts[1], cmd_parts[2..].join(" "))
                    } else if cmd_parts.len() >= 2 {
                        ("", cmd_parts[1..].join(" "))
                    } else {
                        ("", String::new())
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
                    let branch_color = if is_selected { "36" } else { "90" };

                    let selection_marker = if is_selected { "\x1b[36m❯\x1b[0m" } else { " " };

                    if *depth == 0 {
                        // Root level task, minimal indentation
                        if subcommand.is_empty() {
                            output.push_str(&format!(
                                "  {}{} {}  \x1b[{}m{}\x1b[0m \x1b[1;37m{}\x1b[0m\x1b[K\r\n",
                                selection_marker,
                                if is_selected { "\x1b[36m" } else { "" },
                                icon,
                                runner_color,
                                cmd_runner,
                                task_name,
                            ));
                        } else {
                            output.push_str(&format!(
                                "  {}{} {}  \x1b[{}m{}\x1b[0m \x1b[90m{}\x1b[0m \x1b[1;37m{}\x1b[0m\x1b[K\r\n",
                                selection_marker,
                                if is_selected { "\x1b[36m" } else { "" },
                                icon,
                                runner_color,
                                cmd_runner,
                                subcommand,
                                task_name,
                            ));
                        }
                    } else {
                        // Nested task with tree branch
                        if subcommand.is_empty() {
                            output.push_str(&format!(
                                "\x1b[{}m{}{}\x1b[0m {} {}  \x1b[{}m{}\x1b[0m \x1b[1;37m{}\x1b[0m\x1b[K\r\n",
                                branch_color,
                                prefix,
                                branch,
                                selection_marker,
                                icon,
                                runner_color,
                                cmd_runner,
                                task_name,
                            ));
                        } else {
                            output.push_str(&format!(
                                "\x1b[{}m{}{}\x1b[0m {} {}  \x1b[{}m{}\x1b[0m \x1b[90m{}\x1b[0m \x1b[1;37m{}\x1b[0m\x1b[K\r\n",
                                branch_color,
                                prefix,
                                branch,
                                selection_marker,
                                icon,
                                runner_color,
                                cmd_runner,
                                subcommand,
                                task_name,
                            ));
                        }
                    }
                }
            }
        }

        // Status line
        output.push_str("\x1b[K\r\n");
        output.push_str(&format!(
            "\x1b[90m  {}/{} │ ↑↓ navigate │ enter select │ esc cancel\x1b[0m\x1b[K",
            filtered.iter().filter(|(_, i)| matches!(i, PickerItem::Task { .. })).count(),
            all_tasks.len()
        ));

        // Clear any remaining lines below (in case list got shorter)
        output.push_str("\x1b[J");

        // Write all at once
        write!(stdout, "{}", output).ok();
        stdout.flush().ok();

        // Handle input
        if let Ok(Event::Key(key)) = event::read() {
            match key.code {
                KeyCode::Esc => return None,
                KeyCode::Enter => {
                    if let Some((_, PickerItem::Task { idx, .. })) = filtered.get(selected) {
                        return Some(*idx);
                    }
                }
                KeyCode::Up => {
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
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return None;
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    selected = 0;
                    scroll_offset = 0;
                }
                KeyCode::Backspace => {
                    query.pop();
                    selected = 0;
                    scroll_offset = 0;
                }
                _ => {}
            }
        }
    }
}

fn run_task(task: &DisplayTask, root: &PathBuf) {
    println!();
    println!(
        "  {} {} {}",
        task.runner.icon(),
        style("Running").green().bold(),
        style(&task.task.command).white().bold()
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

    let parts: Vec<&str> = task.task.command.split_whitespace().collect();
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
