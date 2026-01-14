//! Parser for package.json (npm/bun/yarn/pnpm scripts)

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::{RunnerType, ScanError, Task, TaskRunner};

use super::Parser;

#[derive(Deserialize)]
struct PackageJson {
    scripts: Option<HashMap<String, String>>,
    #[serde(rename = "packageManager")]
    package_manager: Option<String>,
}

pub struct PackageJsonParser;

impl PackageJsonParser {
    /// Detect the package manager from the packageManager field
    fn detect_runner_type(package_manager: Option<&str>) -> RunnerType {
        match package_manager {
            Some(pm) if pm.starts_with("bun") => RunnerType::Bun,
            Some(pm) if pm.starts_with("yarn") => RunnerType::Yarn,
            Some(pm) if pm.starts_with("pnpm") => RunnerType::Pnpm,
            _ => RunnerType::Npm,
        }
    }

    /// Get the run command prefix for the package manager
    fn run_command(runner_type: RunnerType, script_name: &str) -> String {
        match runner_type {
            RunnerType::Bun => format!("bun run {}", script_name),
            RunnerType::Yarn => format!("yarn {}", script_name),
            RunnerType::Pnpm => format!("pnpm run {}", script_name),
            _ => format!("npm run {}", script_name),
        }
    }
}

impl Parser for PackageJsonParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;

        let pkg: PackageJson = serde_json::from_str(&content).map_err(|e| ScanError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let scripts = match pkg.scripts {
            Some(s) if !s.is_empty() => s,
            _ => return Ok(None),
        };

        let runner_type = Self::detect_runner_type(pkg.package_manager.as_deref());

        let tasks: Vec<Task> = scripts
            .into_iter()
            .map(|(name, script)| Task {
                command: Self::run_command(runner_type, &name),
                name,
                description: None,
                script: Some(script),
            })
            .collect();

        Ok(Some(TaskRunner {
            config_path: path.to_path_buf(),
            runner_type,
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
    fn test_parse_npm_scripts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("package.json");
        fs::write(
            &path,
            r#"{
                "name": "test",
                "scripts": {
                    "build": "tsc",
                    "test": "jest"
                }
            }"#,
        )
        .unwrap();

        let parser = PackageJsonParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Npm);
        assert_eq!(runner.tasks.len(), 2);

        let build_task = runner.tasks.iter().find(|t| t.name == "build").unwrap();
        assert_eq!(build_task.command, "npm run build");
    }

    #[test]
    fn test_parse_bun_scripts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("package.json");
        fs::write(
            &path,
            r#"{
                "name": "test",
                "packageManager": "bun@1.1.38",
                "scripts": {
                    "dev": "bun run dev.ts"
                }
            }"#,
        )
        .unwrap();

        let parser = PackageJsonParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Bun);
        let dev_task = runner.tasks.iter().find(|t| t.name == "dev").unwrap();
        assert_eq!(dev_task.command, "bun run dev");
    }

    #[test]
    fn test_no_scripts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("package.json");
        fs::write(&path, r#"{"name": "test"}"#).unwrap();

        let parser = PackageJsonParser;
        let runner = parser.parse(&path).unwrap();
        assert!(runner.is_none());
    }
}
