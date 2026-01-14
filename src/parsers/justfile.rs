//! Parser for justfile using the `just` crate's summary API

use std::path::Path;

use crate::{RunnerType, ScanError, Task, TaskRunner};

use super::Parser;

pub struct JustfileParser;

impl Parser for JustfileParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        // Use just's summary API to parse the justfile
        let summary = just::summary::summary(path).map_err(|e| ScanError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let summary = match summary {
            Ok(s) => s,
            Err(e) => {
                return Err(ScanError::ParseError {
                    path: path.to_path_buf(),
                    message: e,
                });
            }
        };

        let mut tasks = Vec::new();

        for (name, recipe) in &summary.recipes {
            // Skip private recipes
            if recipe.private || name.starts_with('_') {
                continue;
            }

            tasks.push(Task {
                name: name.clone(),
                command: format!("just {}", name),
                description: None,
            });
        }

        if tasks.is_empty() {
            return Ok(None);
        }

        Ok(Some(TaskRunner {
            config_path: path.to_path_buf(),
            runner_type: RunnerType::Just,
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
    fn test_parse_justfile() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("justfile");
        fs::write(
            &path,
            r#"
# Build the project
build:
    cargo build

# Run tests
test: build
    cargo test

# Private helper
_helper:
    echo "helper"

[private]
internal:
    echo "internal"

deploy env="prod":
    ./deploy.sh {{env}}
"#,
        )
        .unwrap();

        let parser = JustfileParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Just);

        // Should have build, test, deploy but not _helper or internal
        let names: Vec<_> = runner.tasks.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"build"));
        assert!(names.contains(&"test"));
        assert!(names.contains(&"deploy"));
        assert!(!names.contains(&"_helper"));
        assert!(!names.contains(&"internal"));

        let build_task = runner.tasks.iter().find(|t| t.name == "build").unwrap();
        assert_eq!(build_task.command, "just build");
    }

    #[test]
    fn test_empty_justfile() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("justfile");
        fs::write(&path, "# Just a comment\n").unwrap();

        let parser = JustfileParser;
        let runner = parser.parse(&path).unwrap();
        assert!(runner.is_none());
    }
}
