//! Parser for Makefile targets using makefile-lossless

use std::fs;
use std::path::Path;

use makefile_lossless::Makefile;

use crate::{RunnerType, ScanError, Task, TaskRunner};

use super::Parser;

pub struct MakefileParser;

impl MakefileParser {
    /// Check if a target name should be exposed as a runnable task
    fn is_runnable_target(name: &str) -> bool {
        // Skip special targets and internal targets
        !name.starts_with('.') && !name.starts_with('_') && !name.contains('%') && !name.is_empty()
    }
}

impl Parser for MakefileParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;

        let makefile: Makefile = content.parse().map_err(|e| ScanError::ParseError {
            path: path.to_path_buf(),
            message: format!("{:?}", e),
        })?;

        let mut tasks = Vec::new();

        for rule in makefile.rules() {
            for target in rule.targets() {
                if Self::is_runnable_target(&target) {
                    tasks.push(Task {
                        name: target.clone(),
                        command: format!("make {}", target),
                        description: None, // makefile-lossless doesn't expose comments
                        script: None,      // Makefile recipes not easily extractable
                    });
                }
            }
        }

        if tasks.is_empty() {
            return Ok(None);
        }

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
