//! UI thread for terminal interaction

use crate::backend::SharedTasks;
use crate::messages::{SearchRequest, SearchResponse, SelectedTask};
use crate::render::render;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{self, stdout, Write};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

/// Application mode
#[derive(Clone, PartialEq, Debug)]
pub enum Mode {
    Select,
    Edit,
    Expanded,
}

/// UI state
#[derive(Clone)]
pub struct UIState {
    /// Search query
    pub query: String,
    /// Cursor position in query
    pub query_cursor: usize,
    /// Current mode
    pub mode: Mode,
    /// Index of selected task in the matched results list
    pub selected_index: usize,
    /// Scroll offset for viewport
    pub scroll_offset: usize,
    /// Edit buffer for Edit/Expanded modes
    pub edit_buffer: String,
    /// Cursor position in edit buffer
    pub edit_cursor: usize,
}

impl Default for UIState {
    fn default() -> Self {
        Self {
            query: String::new(),
            query_cursor: 0,
            mode: Mode::Select,
            selected_index: 0,
            scroll_offset: 0,
            edit_buffer: String::new(),
            edit_cursor: 0,
        }
    }
}

/// Result from the picker
pub struct PickerResult {
    pub task: SelectedTask,
    pub command: String,
}

/// Result from update
enum UpdateResult {
    Continue(UIState),
    Exit(Option<PickerResult>),
}

/// Run the UI loop
pub fn run(
    request_tx: Sender<SearchRequest>,
    response_rx: Receiver<SearchResponse>,
    tasks: SharedTasks,
    root_name: String,
) -> Option<PickerResult> {
    // Setup terminal
    terminal::enable_raw_mode().ok()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, Hide).ok()?;

    let result = run_ui_loop(request_tx, response_rx, tasks, &root_name, &mut stdout);

    // Restore terminal
    execute!(stdout, Show, LeaveAlternateScreen).ok();
    terminal::disable_raw_mode().ok();

    result
}

/// Main UI loop
fn run_ui_loop(
    request_tx: Sender<SearchRequest>,
    response_rx: Receiver<SearchResponse>,
    tasks: SharedTasks,
    root_name: &str,
    stdout: &mut io::Stdout,
) -> Option<PickerResult> {
    let mut state = UIState::default();
    let mut last_response: Option<SearchResponse> = None;
    let mut needs_search = true;

    loop {
        let (_, height) = terminal::size().unwrap_or((80, 24));
        let viewport_height = (height as usize).saturating_sub(8);

        // Send search request if needed
        if needs_search {
            let request = SearchRequest {
                query: state.query.clone(),
                offset: state.scroll_offset,
                limit: viewport_height * 2,
                viewport_lines: viewport_height,
                selected_index: state.selected_index,
            };
            if request_tx.send(request).is_err() {
                return None;
            }
            needs_search = false;
        }

        // Try to receive response
        match response_rx.try_recv() {
            Ok(response) => {
                let task_count = response.matched_tasks;

                // Update selection to stay within bounds
                if task_count > 0 {
                    state.selected_index = state.selected_index.min(task_count - 1);
                } else {
                    state.selected_index = 0;
                }

                // Use backend's corrected scroll offset
                state.scroll_offset = response.offset;

                // If scanning is still in progress, request another update
                if !response.scanning_done {
                    needs_search = true;
                }

                last_response = Some(response);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                return None;
            }
        }

        // Poll for keyboard input
        if event::poll(Duration::from_millis(50)).unwrap_or(false) {
            if let Ok(CrosstermEvent::Key(key)) = event::read() {
                let task_count = last_response.as_ref().map(|r| r.matched_tasks).unwrap_or(0);

                let selected_task = last_response.as_ref().and_then(|r| {
                    let relative_idx = state.selected_index.saturating_sub(r.offset);
                    get_selected_task(&tasks, &r.matched_indices, relative_idx)
                });

                match handle_key(state.clone(), key, selected_task.as_ref(), task_count) {
                    UpdateResult::Continue(new_state) => {
                        let query_changed = new_state.query != state.query;
                        state = new_state;

                        if query_changed {
                            state.selected_index = 0;
                            state.scroll_offset = 0;
                        }
                        // Request new data - backend will calculate correct scroll
                        needs_search = true;
                    }
                    UpdateResult::Exit(result) => return result,
                }
            }
        }

        // Render current state
        if let Some(ref response) = last_response {
            execute!(stdout, MoveTo(0, 0)).ok();
            let result = render(&state, response, &tasks, root_name, height as usize);
            write!(stdout, "{}", result.output).ok();
            stdout.flush().ok();
        }
    }
}

/// Get selected task from shared storage
fn get_selected_task(
    tasks: &SharedTasks,
    matched_indices: &[u32],
    selected_index: usize,
) -> Option<SelectedTask> {
    if selected_index >= matched_indices.len() {
        return None;
    }
    let idx = matched_indices[selected_index] as usize;
    let tasks = tasks.read().ok()?;
    tasks.get(idx).map(SelectedTask::from)
}

/// Handle a key event
fn handle_key(
    state: UIState,
    key: KeyEvent,
    selected_task: Option<&SelectedTask>,
    task_count: usize,
) -> UpdateResult {
    match key.code {
        // Ctrl+C always exits
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            UpdateResult::Exit(None)
        }

        // Escape: go back one step (Expanded → Edit → Select → Exit)
        KeyCode::Esc => match state.mode {
            Mode::Expanded => {
                // Go back to Edit with original command
                let command = selected_task.map(|t| t.command.clone()).unwrap_or_default();
                UpdateResult::Continue(UIState {
                    mode: Mode::Edit,
                    edit_buffer: command.clone(),
                    edit_cursor: command.len(),
                    ..state
                })
            }
            Mode::Edit => UpdateResult::Continue(UIState {
                mode: Mode::Select,
                ..state
            }),
            Mode::Select => UpdateResult::Exit(None),
        },

        // Run selected task
        KeyCode::Enter => {
            if let Some(task) = selected_task {
                let command = if matches!(state.mode, Mode::Edit | Mode::Expanded) {
                    state.edit_buffer.clone()
                } else {
                    task.command.clone()
                };
                return UpdateResult::Exit(Some(PickerResult {
                    task: task.clone(),
                    command,
                }));
            }
            UpdateResult::Continue(state)
        }

        // Tab: cycle through modes (Select → Edit → Expanded → Select)
        KeyCode::Tab => match state.mode {
            Mode::Select => {
                if let Some(task) = selected_task {
                    let command = task.command.clone();
                    UpdateResult::Continue(UIState {
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
                if let Some(task) = selected_task {
                    if let Some(ref script) = task.script {
                        return UpdateResult::Continue(UIState {
                            mode: Mode::Expanded,
                            edit_buffer: script.clone(),
                            edit_cursor: script.len(),
                            ..state
                        });
                    }
                }
                UpdateResult::Continue(state)
            }
            Mode::Expanded => UpdateResult::Continue(state),
        },

        // Navigation
        KeyCode::Up => {
            let new_idx = move_selection(state.selected_index, task_count, -1);
            UpdateResult::Continue(UIState {
                mode: Mode::Select,
                selected_index: new_idx,
                ..state
            })
        }
        KeyCode::Down => {
            let new_idx = move_selection(state.selected_index, task_count, 1);
            UpdateResult::Continue(UIState {
                mode: Mode::Select,
                selected_index: new_idx,
                ..state
            })
        }

        // Text input
        _ => {
            if matches!(state.mode, Mode::Edit | Mode::Expanded) {
                let (new_buffer, new_cursor) =
                    apply_input_event(&state.edit_buffer, state.edit_cursor, key);
                UpdateResult::Continue(UIState {
                    edit_buffer: new_buffer,
                    edit_cursor: new_cursor,
                    ..state
                })
            } else {
                let (new_query, new_cursor) =
                    apply_input_event(&state.query, state.query_cursor, key);
                let query_changed = new_query != state.query;
                UpdateResult::Continue(UIState {
                    query: new_query,
                    query_cursor: new_cursor,
                    selected_index: if query_changed {
                        0
                    } else {
                        state.selected_index
                    },
                    scroll_offset: if query_changed {
                        0
                    } else {
                        state.scroll_offset
                    },
                    ..state
                })
            }
        }
    }
}

/// Move selection with wrap-around
fn move_selection(current: usize, total: usize, delta: isize) -> usize {
    if total == 0 {
        return 0;
    }
    ((current as isize + delta).rem_euclid(total as isize)) as usize
}

/// Apply a key event to a text buffer
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_move_selection_wrap() {
        assert_eq!(move_selection(0, 5, -1), 4);
        assert_eq!(move_selection(4, 5, 1), 0);
        assert_eq!(move_selection(2, 5, 1), 3);
    }

    #[test]
    fn test_apply_input_char() {
        let (buffer, cursor) = apply_input_event(
            "hello",
            2,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert_eq!(buffer, "hexllo");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn test_apply_input_backspace() {
        let (buffer, cursor) = apply_input_event(
            "hello",
            2,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(buffer, "hllo");
        assert_eq!(cursor, 1);
    }
}
