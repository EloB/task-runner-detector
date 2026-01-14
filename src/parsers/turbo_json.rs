//! Parser for turbo.json (Turborepo)

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::{RunnerType, ScanError, Task, TaskRunner};

use super::Parser;

#[derive(Deserialize)]
struct TurboJson {
    // v2 format
    tasks: Option<HashMap<String, serde_json::Value>>,
    // v1 format (legacy)
    pipeline: Option<HashMap<String, serde_json::Value>>,
}

pub struct TurboJsonParser;

impl Parser for TurboJsonParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;

        let turbo: TurboJson = serde_json::from_str(&content).map_err(|e| ScanError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        // Prefer v2 tasks, fall back to v1 pipeline
        let task_map = turbo.tasks.or(turbo.pipeline);

        let task_map = match task_map {
            Some(t) if !t.is_empty() => t,
            _ => return Ok(None),
        };

        let tasks: Vec<Task> = task_map
            .keys()
            .filter(|name| !name.starts_with('/')) // Skip workspace-specific tasks
            .map(|name| Task {
                name: name.clone(),
                command: format!("turbo run {}", name),
                description: Some("Turborepo task (runs across workspaces)".to_string()),
                script: None,
            })
            .collect();

        if tasks.is_empty() {
            return Ok(None);
        }

        Ok(Some(TaskRunner {
            config_path: path.to_path_buf(),
            runner_type: RunnerType::Turbo,
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
    fn test_parse_turbo_v2() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("turbo.json");
        fs::write(
            &path,
            r#"{
                "$schema": "https://turbo.build/schema.json",
                "tasks": {
                    "build": { "dependsOn": ["^build"] },
                    "test": { "dependsOn": ["build"] },
                    "lint": {}
                }
            }"#,
        )
        .unwrap();

        let parser = TurboJsonParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Turbo);
        assert_eq!(runner.tasks.len(), 3);

        let build_task = runner.tasks.iter().find(|t| t.name == "build").unwrap();
        assert_eq!(build_task.command, "turbo run build");
    }

    #[test]
    fn test_parse_turbo_v1() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("turbo.json");
        fs::write(
            &path,
            r#"{
                "pipeline": {
                    "build": { "dependsOn": ["^build"] },
                    "dev": { "cache": false }
                }
            }"#,
        )
        .unwrap();

        let parser = TurboJsonParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.tasks.len(), 2);
    }
}
