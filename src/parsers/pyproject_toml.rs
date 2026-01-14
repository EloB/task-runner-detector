//! Parser for pyproject.toml (Poetry, PDM, PEP 621)

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use toml::Value;

use crate::{RunnerType, ScanError, Task, TaskRunner};

use super::Parser;

#[derive(Deserialize)]
struct PyprojectToml {
    tool: Option<Tool>,
    project: Option<Project>,
}

#[derive(Deserialize)]
struct Tool {
    poetry: Option<PoetryConfig>,
    pdm: Option<PdmConfig>,
}

#[derive(Deserialize)]
struct PoetryConfig {
    scripts: Option<HashMap<String, Value>>,
}

#[derive(Deserialize)]
struct PdmConfig {
    scripts: Option<HashMap<String, Value>>,
}

#[derive(Deserialize)]
struct Project {
    scripts: Option<HashMap<String, String>>,
}

pub struct PyprojectTomlParser;

impl PyprojectTomlParser {
    fn extract_script_command(value: &Value) -> Option<String> {
        match value {
            Value::String(s) => Some(s.clone()),
            Value::Table(t) => {
                // Handle {call = "module:func"} or {cmd = "command"} format
                t.get("call")
                    .or_else(|| t.get("cmd"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }
            _ => None,
        }
    }
}

impl Parser for PyprojectTomlParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;

        let pyproject: PyprojectToml =
            toml::from_str(&content).map_err(|e| ScanError::ParseError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        let mut tasks = Vec::new();
        let mut runner_type = RunnerType::Poetry; // Default, will be updated

        // Check for Poetry scripts
        if let Some(tool) = &pyproject.tool {
            if let Some(poetry) = &tool.poetry {
                if let Some(scripts) = &poetry.scripts {
                    runner_type = RunnerType::Poetry;
                    for (name, value) in scripts {
                        if let Some(cmd) = Self::extract_script_command(value) {
                            tasks.push(Task {
                                name: name.clone(),
                                command: format!("poetry run {}", name),
                                description: Some(cmd),
                            });
                        }
                    }
                }
            }

            // Check for PDM scripts (takes precedence if both exist)
            if let Some(pdm) = &tool.pdm {
                if let Some(scripts) = &pdm.scripts {
                    runner_type = RunnerType::Pdm;
                    tasks.clear(); // Clear poetry tasks if PDM is found
                    for (name, value) in scripts {
                        if let Some(cmd) = Self::extract_script_command(value) {
                            tasks.push(Task {
                                name: name.clone(),
                                command: format!("pdm run {}", name),
                                description: Some(cmd),
                            });
                        }
                    }
                }
            }
        }

        // Check for PEP 621 project.scripts (entry points)
        if let Some(project) = &pyproject.project {
            if let Some(scripts) = &project.scripts {
                for (name, entry_point) in scripts {
                    tasks.push(Task {
                        name: name.clone(),
                        command: name.clone(), // Entry points are installed as commands
                        description: Some(format!("Entry point: {}", entry_point)),
                    });
                }
            }
        }

        if tasks.is_empty() {
            return Ok(None);
        }

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
    fn test_parse_poetry_scripts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pyproject.toml");
        fs::write(
            &path,
            r#"
[tool.poetry]
name = "myproject"

[tool.poetry.scripts]
test = "pytest"
lint = "ruff check ."
"#,
        )
        .unwrap();

        let parser = PyprojectTomlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Poetry);
        assert_eq!(runner.tasks.len(), 2);

        let test_task = runner.tasks.iter().find(|t| t.name == "test").unwrap();
        assert_eq!(test_task.command, "poetry run test");
    }

    #[test]
    fn test_parse_pdm_scripts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pyproject.toml");
        fs::write(
            &path,
            r#"
[tool.pdm.scripts]
start = "python main.py"
test = { cmd = "pytest -v" }
"#,
        )
        .unwrap();

        let parser = PyprojectTomlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Pdm);
        assert!(runner.tasks.iter().any(|t| t.name == "start"));
    }

    #[test]
    fn test_parse_pep621_scripts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pyproject.toml");
        fs::write(
            &path,
            r#"
[project]
name = "myproject"

[project.scripts]
mycli = "myproject.cli:main"
"#,
        )
        .unwrap();

        let parser = PyprojectTomlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert!(runner.tasks.iter().any(|t| t.name == "mycli"));
    }
}
