//! Parser for deno.json / deno.jsonc (Deno tasks)

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::{RunnerType, ScanError, Task, TaskRunner};

use super::Parser;

#[derive(Deserialize)]
struct DenoJson {
    tasks: Option<HashMap<String, TaskConfig>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TaskConfig {
    Simple(String),
    Complex {
        command: Option<String>,
        description: Option<String>,
    },
}

pub struct DenoJsonParser;

impl DenoJsonParser {
    /// Strip JSONC comments from content
    fn strip_jsonc_comments(content: &str) -> String {
        let mut result = String::with_capacity(content.len());
        let mut in_string = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        let mut chars = content.chars().peekable();

        while let Some(c) = chars.next() {
            if in_line_comment {
                if c == '\n' {
                    in_line_comment = false;
                    result.push(c);
                }
                continue;
            }

            if in_block_comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    in_block_comment = false;
                }
                continue;
            }

            if in_string {
                result.push(c);
                if c == '\\' {
                    if let Some(next) = chars.next() {
                        result.push(next);
                    }
                } else if c == '"' {
                    in_string = false;
                }
                continue;
            }

            // Not in string or comment
            if c == '"' {
                in_string = true;
                result.push(c);
            } else if c == '/' {
                match chars.peek() {
                    Some('/') => {
                        chars.next();
                        in_line_comment = true;
                    }
                    Some('*') => {
                        chars.next();
                        in_block_comment = true;
                    }
                    _ => result.push(c),
                }
            } else {
                result.push(c);
            }
        }

        result
    }
}

impl Parser for DenoJsonParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;

        // Handle JSONC (JSON with comments)
        let content = if path.extension().map(|e| e == "jsonc").unwrap_or(false) {
            Self::strip_jsonc_comments(&content)
        } else {
            content
        };

        let deno: DenoJson = serde_json::from_str(&content).map_err(|e| ScanError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let task_map = match deno.tasks {
            Some(t) if !t.is_empty() => t,
            _ => return Ok(None),
        };

        let tasks: Vec<Task> = task_map
            .into_iter()
            .map(|(name, config)| {
                let (command_str, description) = match config {
                    TaskConfig::Simple(cmd) => (cmd, None),
                    TaskConfig::Complex {
                        command,
                        description,
                    } => (command.unwrap_or_default(), description),
                };

                Task {
                    command: format!("deno task {}", name),
                    description: if description.is_some() {
                        description
                    } else {
                        Some(command_str.clone())
                    },
                    name,
                    script: Some(command_str),
                }
            })
            .collect();

        if tasks.is_empty() {
            return Ok(None);
        }

        Ok(Some(TaskRunner {
            config_path: path.to_path_buf(),
            runner_type: RunnerType::Deno,
            tasks,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_deno_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deno.json");
        fs::write(
            &path,
            r#"{
                "tasks": {
                    "dev": "deno run --watch main.ts",
                    "build": "deno compile main.ts"
                }
            }"#,
        )
        .unwrap();

        let parser = DenoJsonParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Deno);
        assert_eq!(runner.tasks.len(), 2);

        let dev_task = runner.tasks.iter().find(|t| t.name == "dev").unwrap();
        assert_eq!(dev_task.command, "deno task dev");
    }

    #[test]
    fn test_parse_deno_jsonc() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deno.jsonc");
        fs::write(
            &path,
            r#"{
                // This is a comment
                "tasks": {
                    "start": "deno run main.ts" /* inline comment */
                }
            }"#,
        )
        .unwrap();

        let parser = DenoJsonParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.tasks.len(), 1);
        assert_eq!(runner.tasks[0].name, "start");
    }

    #[test]
    fn test_no_tasks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("deno.json");
        fs::write(&path, r#"{"imports": {}}"#).unwrap();

        let parser = DenoJsonParser;
        let runner = parser.parse(&path).unwrap();
        assert!(runner.is_none());
    }
}
