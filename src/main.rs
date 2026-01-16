//! Task CLI - Discover and run tasks from various config files
//!
//! Usage:
//!   task                    # Interactive picker (scan cwd, select, run)
//!   task <path>             # Interactive picker for specific directory
//!   task -j                 # JSON output
//!   task -s                 # Streaming NDJSON output
//!   task -j -q "query"      # Filter JSON output with fuzzy search

use std::env;
use std::io::{stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};

use clap::Parser;
use console::style;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

use task_runner_detector::{
    scan_streaming, scan_with_options, RunnerType, ScanOptions, Task, TaskRunner,
};

mod backend;
mod messages;
mod registry;
mod render;
mod ui;

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

/// Filter a single runner's tasks by query
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
            let haystack = Utf32Str::new(&search_text, &mut buf);
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
    let mut matcher = Matcher::new(Config::DEFAULT);

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

    // JSON array output mode
    if cli.json {
        let runners = scan_with_options(&root, options.clone()).unwrap_or_default();
        let runners = filter_runners_by_query(runners, cli.query.as_deref(), &root);
        println!(
            "{}",
            serde_json::to_string_pretty(&runners).unwrap_or_else(|_| "[]".into())
        );
        return;
    }

    // NDJSON streaming output mode
    if cli.json_stream {
        let (tx, rx) = mpsc::channel();
        let _scanner_handle = scan_streaming(root.clone(), options, tx);

        let mut stdout = stdout().lock();
        let mut matcher = Matcher::new(Config::DEFAULT);
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

    // Interactive mode - use new UI/Backend architecture
    let (request_tx, request_rx) = mpsc::channel();
    let (response_tx, response_rx) = mpsc::channel();

    // Create shared task storage
    let tasks = Arc::new(RwLock::new(Vec::new()));

    // Get root name for display
    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    // Spawn backend thread
    let _backend_handle = backend::spawn_backend(
        root.clone(),
        options,
        tasks.clone(),
        request_rx,
        response_tx,
    );

    // Run UI on main thread
    match ui::run(request_tx, response_rx, tasks, root_name) {
        Some(result) => {
            run_task(&result.task, &result.command, &root);
        }
        None => {
            println!();
            println!("  {} Cancelled", style("✗").dim());
        }
    }
}

/// Run a task
fn run_task(task: &messages::SelectedTask, command: &str, root: &Path) {
    let work_dir = task.config_path.parent().unwrap_or(root);
    let sep = style("─".repeat(60)).dim();

    println!(
        "\n  {} {} {}",
        task.runner_type.icon(),
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
    use crate::backend::{Backend, SharedTasks};
    use crate::messages::SearchRequest;
    use crate::render::render;
    use crate::ui::{Mode, UIState};

    /// Test that the first render matches the expected output
    #[test]
    fn test_first_render_matches_expected() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let root = PathBuf::from(manifest_dir);

        // Scan the project root
        let runners = task_runner_detector::scan_with_options(&root, ScanOptions::default())
            .unwrap_or_default();

        // Create shared task storage
        let tasks: SharedTasks = Arc::new(RwLock::new(Vec::new()));

        // Build a backend and populate it with tasks
        let mut backend = Backend::new(root.clone(), tasks.clone());
        for runner in &runners {
            backend.add_runner_for_test(runner.clone());
        }

        // Create a search request
        let request = SearchRequest {
            query: String::new(),
            offset: 0,
            limit: 100,
            viewport_lines: 30,
            selected_index: 0,
        };

        // Get search response
        let response = backend.handle_search_for_test(request);

        // Create UI state with first task selected
        let state = UIState {
            query: String::new(),
            query_cursor: 0,
            mode: Mode::Select,
            selected_index: 0,
            scroll_offset: 0,
            edit_buffer: String::new(),
            edit_cursor: 0,
        };

        // Get root name for display
        let root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        // Render
        let result = render(&state, &response, &tasks, &root_name, 50);

        // Read expected output and compare
        let expected_path = root.join("fixtures/first_render.txt");

        // Allow updating the fixture with UPDATE_FIXTURES=1 cargo test
        if std::env::var("UPDATE_FIXTURES").is_ok() {
            std::fs::write(&expected_path, &result.output).expect("Failed to write fixture");
            return;
        }

        let expected = std::fs::read_to_string(&expected_path)
            .expect("Failed to read fixtures/first_render.txt");

        assert_eq!(
            result.output, expected,
            "Render output doesn't match expected fixture"
        );
    }
}
