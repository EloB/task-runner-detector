//! ANSI rendering for the terminal UI

use crate::backend::SharedTasks;
use crate::messages::{SearchResponse, TaskItem};
use crate::ui::{Mode, UIState};

/// Display item for rendering
pub enum DisplayItem<'a> {
    Folder {
        name: &'a str,
        depth: usize,
        is_last: bool,
        parent_is_last: Vec<bool>,
    },
    Task {
        task: &'a TaskItem,
        depth: usize,
        is_last: bool,
        parent_is_last: Vec<bool>,
    },
}

/// Build display items from matched indices and shared tasks
pub fn build_display_items<'a>(
    tasks: &'a [TaskItem],
    matched_indices: &[u32],
    root_name: &'a str,
) -> Vec<DisplayItem<'a>> {
    if matched_indices.is_empty() {
        return vec![];
    }

    let mut items = Vec::new();
    let mut current_folder: Option<&str> = None;
    let mut folder_stack: Vec<(&str, bool)> = vec![]; // (folder_segment, is_last)

    // Group indices by folder to determine last items
    let mut folder_groups: Vec<(&str, Vec<u32>)> = Vec::new();
    for &idx in matched_indices {
        let task = &tasks[idx as usize];
        let folder = task.folder.as_str();
        if folder_groups.last().map(|(f, _)| *f) != Some(folder) {
            folder_groups.push((folder, vec![idx]));
        } else {
            folder_groups.last_mut().unwrap().1.push(idx);
        }
    }

    // Emit root folder
    items.push(DisplayItem::Folder {
        name: root_name,
        depth: 0,
        is_last: true,
        parent_is_last: vec![],
    });

    for (group_idx, (folder, task_indices)) in folder_groups.iter().enumerate() {
        let is_last_folder_group = group_idx == folder_groups.len() - 1;

        // Emit folder headers if folder changed
        if current_folder != Some(folder) {
            // Determine folder path segments
            let segments: Vec<&str> = if *folder == "." {
                vec![]
            } else {
                folder.split('/').collect()
            };

            // Find common prefix with current folder stack
            let common_len = folder_stack
                .iter()
                .zip(segments.iter())
                .take_while(|((a, _), b)| *a == **b)
                .count();

            // Pop folders that are no longer in path
            folder_stack.truncate(common_len);

            // Push new folders
            for (i, &segment) in segments.iter().enumerate().skip(common_len) {
                let depth = i + 1;

                // Check if this folder segment is the last at its depth
                // by looking at remaining folder groups for any that would have a different
                // folder at this depth (i.e., a sibling folder)
                let is_last_at_depth =
                    !folder_groups[group_idx + 1..]
                        .iter()
                        .any(|(other_folder, _)| {
                            let other_segments: Vec<&str> = if *other_folder == "." {
                                vec![]
                            } else {
                                other_folder.split('/').collect()
                            };
                            // Check if other folder has a different segment at this depth
                            // meaning it's a sibling, not a descendant
                            other_segments.len() >= depth
                                && other_segments[..depth - 1] == segments[..depth - 1]
                                && (other_segments.len() == depth
                                    || other_segments[depth - 1] != segment)
                        });

                folder_stack.push((segment, is_last_at_depth));

                let parent_is_last: Vec<bool> = folder_stack[..folder_stack.len() - 1]
                    .iter()
                    .map(|(_, is_last)| *is_last)
                    .collect();

                items.push(DisplayItem::Folder {
                    name: segment,
                    depth,
                    is_last: is_last_at_depth,
                    parent_is_last,
                });
            }

            current_folder = Some(folder);
        }

        // Emit tasks in this folder
        let task_depth = if *folder == "." {
            1
        } else {
            folder.split('/').count() + 1
        };

        for (task_idx_in_group, &idx) in task_indices.iter().enumerate() {
            let task = &tasks[idx as usize];
            let is_last_task = task_idx_in_group == task_indices.len() - 1 && is_last_folder_group;

            let parent_is_last: Vec<bool> =
                folder_stack.iter().map(|(_, is_last)| *is_last).collect();

            items.push(DisplayItem::Task {
                task,
                depth: task_depth,
                is_last: is_last_task,
                parent_is_last,
            });
        }
    }

    items
}

/// Compute tree prefix from depth and parent_is_last info
fn tree_prefix(depth: usize, is_last: bool, parent_is_last: &[bool]) -> String {
    if depth == 0 {
        return "  ".to_string();
    }

    let mut prefix = String::from("  ");
    for &parent_last in parent_is_last {
        prefix.push_str(if parent_last { "   " } else { "│  " });
    }
    prefix.push_str(if is_last { "└─" } else { "├─" });
    prefix
}

/// Render result containing the output string and metadata
pub struct RenderResult {
    pub output: String,
    /// Number of tasks that were actually rendered (not including folder headers)
    pub tasks_rendered: usize,
}

/// Render the entire UI to a string
pub fn render(
    state: &UIState,
    response: &SearchResponse,
    tasks: &SharedTasks,
    root_name: &str,
    terminal_height: usize,
) -> RenderResult {
    let mut output = String::new();

    // Header
    output.push_str("\x1b[36m  Task Runner Detector\x1b[0m");
    if !response.scanning_done {
        output.push_str(" \x1b[33m(scanning...)\x1b[0m");
    }
    output.push_str("\x1b[K\r\n");
    output.push_str(&format!(
        "\x1b[90m  {} tasks found\x1b[0m\x1b[K\r\n",
        response.total_tasks
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

    // Build display items from shared tasks
    // matched_indices is a slice starting at response.offset
    let tasks_guard = tasks.read().unwrap();
    let display_items = build_display_items(&tasks_guard, &response.matched_indices, root_name);

    // The selected_index is absolute, convert to relative within this slice
    let relative_selected = state.selected_index.saturating_sub(response.offset);

    // Render all display items (they're already the viewport slice from backend)
    let list_height = terminal_height.saturating_sub(8);
    let mut task_idx = 0;
    for (rendered_lines, item) in display_items.iter().enumerate() {
        if rendered_lines >= list_height {
            break;
        }
        let is_selected = matches!(item, DisplayItem::Task { .. }) && task_idx == relative_selected;
        output.push_str(&render_item(item, is_selected, state));
        if matches!(item, DisplayItem::Task { .. }) {
            task_idx += 1;
        }
    }

    // Status line
    output.push_str("\x1b[K\r\n");
    let task_count = response.matched_tasks;
    let current_task_num = if task_count > 0 {
        (state.selected_index + 1).min(task_count)
    } else {
        0
    };

    match state.mode {
        Mode::Select => output.push_str(&format!(
            "\x1b[90m  {}/{} │ ↑↓ navigate │ tab edit │ enter run │ esc cancel\x1b[0m\x1b[K",
            current_task_num, task_count
        )),
        Mode::Edit => output.push_str(
            "\x1b[90m  edit mode │ ↑↓ back to select │ tab expand │ enter run │ esc cancel\x1b[0m\x1b[K",
        ),
        Mode::Expanded => output.push_str(
            "\x1b[90m  expanded │ ↑↓ back to select │ tab back │ enter run │ esc cancel\x1b[0m\x1b[K",
        ),
    }

    output.push_str("\x1b[J");
    RenderResult {
        output,
        tasks_rendered: task_idx,
    }
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

/// Render a single display item
fn render_item(item: &DisplayItem, is_selected: bool, state: &UIState) -> String {
    match item {
        DisplayItem::Folder {
            name,
            depth,
            is_last,
            parent_is_last,
        } => {
            let prefix = tree_prefix(*depth, *is_last, parent_is_last);
            if *depth == 0 {
                format!("  📁 \x1b[1;37m{}\x1b[0m\x1b[K\r\n", name)
            } else {
                format!(
                    "\x1b[90m{}\x1b[0m 📁 \x1b[1;37m{}\x1b[0m\x1b[K\r\n",
                    prefix, name
                )
            }
        }
        DisplayItem::Task {
            task,
            depth,
            is_last,
            parent_is_last,
        } => {
            let prefix = tree_prefix(*depth, *is_last, parent_is_last);
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
                format!("\x1b[90m{}\x1b[0m", task.command)
            } else {
                render_command_highlighted(&task.command, &[])
            };

            let branch_color = if is_selected { "36" } else { "90" };
            let icon = task.runner_icon();

            if is_dimmed {
                format!(
                    "\x1b[90m{}\x1b[0m {} \x1b[90m{}\x1b[0m  {}\x1b[K\r\n",
                    prefix, marker, icon, cmd
                )
            } else {
                format!(
                    "\x1b[{}m{}\x1b[0m {} {}  {}\x1b[K\r\n",
                    branch_color, prefix, marker, icon, cmd
                )
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_input_cursor_middle() {
        let (before, cursor, after) = render_input_cursor("hello", 2);
        assert_eq!(before, "he");
        assert_eq!(cursor, 'l');
        assert_eq!(after, "lo");
    }

    #[test]
    fn test_render_input_cursor_end() {
        let (before, cursor, after) = render_input_cursor("hello", 5);
        assert_eq!(before, "hello");
        assert_eq!(cursor, ' ');
        assert_eq!(after, "");
    }

    #[test]
    fn test_render_command_highlighted() {
        let result = render_command_highlighted("npm run build", &[]);
        // Should contain color codes
        assert!(result.contains("\x1b[36m")); // Cyan for npm
        assert!(result.contains("\x1b[90m")); // Gray for run
        assert!(result.contains("\x1b[37m")); // White for build
    }

    #[test]
    fn test_tree_prefix() {
        // Root level
        assert_eq!(tree_prefix(0, true, &[]), "  ");

        // First level, not last
        assert_eq!(tree_prefix(1, false, &[]), "  ├─");

        // First level, last
        assert_eq!(tree_prefix(1, true, &[]), "  └─");

        // Second level, parent not last
        assert_eq!(tree_prefix(2, false, &[false]), "  │  ├─");

        // Second level, parent is last
        assert_eq!(tree_prefix(2, false, &[true]), "     ├─");
    }
}
