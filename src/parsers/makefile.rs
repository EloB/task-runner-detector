//! Simple parser for Makefile targets (thread-safe, no external deps)

use std::fs;
use std::path::Path;

use crate::{RunnerType, ScanError, Task, TaskRunner};

use super::Parser;

pub struct MakefileParser;

impl MakefileParser {
    /// Check if a target name should be exposed as a runnable task
    fn is_runnable_target(name: &str) -> bool {
        !name.starts_with('.') && !name.starts_with('_') && !name.contains('%') && !name.is_empty()
    }

    /// Parse targets from makefile content
    fn parse_targets(content: &str) -> Vec<String> {
        let mut targets = Vec::new();
        for line in content.lines() {
            // Skip empty lines, comments, and lines starting with whitespace (recipes)
            let trimmed = line.trim_start();
            if trimmed.is_empty()
                || trimmed.starts_with('#')
                || line.starts_with('\t')
                || line.starts_with(' ')
            {
                continue;
            }
            // Look for target definitions: "target:" or "target: deps"
            if let Some(colon_pos) = line.find(':') {
                // Skip := and ::= (variable assignments)
                if line[colon_pos..].starts_with(":=") || line[colon_pos..].starts_with("::=") {
                    continue;
                }
                let target_part = &line[..colon_pos];
                // Handle multiple targets on same line: "foo bar: deps"
                for target in target_part.split_whitespace() {
                    if Self::is_runnable_target(target) && !targets.contains(&target.to_string()) {
                        targets.push(target.to_string());
                    }
                }
            }
        }
        targets
    }
}

impl Parser for MakefileParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;
        let targets = Self::parse_targets(&content);

        if targets.is_empty() {
            return Ok(None);
        }

        let tasks = targets
            .into_iter()
            .map(|name| Task {
                command: format!("make {}", name),
                name,
                description: None,
                script: None,
            })
            .collect();

        Ok(Some(TaskRunner {
            config_path: path.to_path_buf(),
            runner_type: RunnerType::Make,
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
    fn test_parse_makefile() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("Makefile");
        fs::write(
            &path,
            r#"
.PHONY: build test

build:
	cargo build

test: build
	cargo test

clean:
	rm -rf target
"#,
        )
        .unwrap();

        let parser = MakefileParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Make);
        assert_eq!(runner.tasks.len(), 3);

        let build_task = runner.tasks.iter().find(|t| t.name == "build").unwrap();
        assert_eq!(build_task.command, "make build");
    }

    #[test]
    fn test_skip_pattern_rules() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("Makefile");
        fs::write(
            &path,
            r#"
%.o: %.c
	$(CC) -c $<

build:
	echo build
"#,
        )
        .unwrap();

        let parser = MakefileParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        // Should only have "build", not the pattern rule
        assert_eq!(runner.tasks.len(), 1);
        assert_eq!(runner.tasks[0].name, "build");
    }
}
