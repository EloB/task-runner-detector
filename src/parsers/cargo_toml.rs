//! Parser for Cargo.toml (cargo binaries and cargo-make scripts)

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::{RunnerType, ScanError, Task, TaskRunner};

use super::Parser;

#[derive(Deserialize)]
struct CargoToml {
    package: Option<Package>,
    bin: Option<Vec<BinTarget>>,
}

#[derive(Deserialize)]
struct Package {
    name: Option<String>,
    metadata: Option<PackageMetadata>,
}

#[derive(Deserialize)]
struct PackageMetadata {
    scripts: Option<HashMap<String, String>>,
}

#[derive(Deserialize)]
struct BinTarget {
    name: String,
}

pub struct CargoTomlParser;

impl Parser for CargoTomlParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;

        let cargo: CargoToml = toml::from_str(&content).map_err(|e| ScanError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let mut tasks = Vec::new();

        // Check for custom scripts in [package.metadata.scripts]
        if let Some(pkg) = &cargo.package {
            if let Some(metadata) = &pkg.metadata {
                if let Some(scripts) = &metadata.scripts {
                    for (name, command) in scripts {
                        tasks.push(Task {
                            name: name.clone(),
                            command: command.clone(),
                            description: None,
                            script: Some(command.clone()),
                        });
                    }
                }
            }
        }

        // Check for [[bin]] targets
        if let Some(bins) = cargo.bin {
            for bin in bins {
                tasks.push(Task {
                    name: bin.name.clone(),
                    command: format!("cargo run --bin {}", bin.name),
                    description: Some(format!("Run the {} binary", bin.name)),
                    script: None,
                });
            }
        }

        // Add default cargo commands if this is a package (has a name)
        if let Some(pkg) = &cargo.package {
            if pkg.name.is_some() {
                // Only add if no other tasks (to avoid cluttering)
                if tasks.is_empty() {
                    tasks.push(Task {
                        name: "build".to_string(),
                        command: "cargo build".to_string(),
                        description: Some("Build the package".to_string()),
                        script: None,
                    });
                    tasks.push(Task {
                        name: "test".to_string(),
                        command: "cargo test".to_string(),
                        description: Some("Run tests".to_string()),
                        script: None,
                    });
                    tasks.push(Task {
                        name: "run".to_string(),
                        command: "cargo run".to_string(),
                        description: Some("Run the package".to_string()),
                        script: None,
                    });
                }
            }
        }

        if tasks.is_empty() {
            return Ok(None);
        }

        Ok(Some(TaskRunner {
            config_path: path.to_path_buf(),
            runner_type: RunnerType::Cargo,
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
    fn test_parse_cargo_with_bins() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("Cargo.toml");
        fs::write(
            &path,
            r#"
[package]
name = "myapp"
version = "0.1.0"

[[bin]]
name = "server"

[[bin]]
name = "cli"
"#,
        )
        .unwrap();

        let parser = CargoTomlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Cargo);
        assert_eq!(runner.tasks.len(), 2);

        let server_task = runner.tasks.iter().find(|t| t.name == "server").unwrap();
        assert_eq!(server_task.command, "cargo run --bin server");
    }

    #[test]
    fn test_parse_cargo_with_scripts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("Cargo.toml");
        fs::write(
            &path,
            r#"
[package]
name = "myapp"
version = "0.1.0"

[package.metadata.scripts]
dev = "cargo watch -x run"
lint = "cargo clippy -- -D warnings"
"#,
        )
        .unwrap();

        let parser = CargoTomlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.tasks.len(), 2);

        let dev_task = runner.tasks.iter().find(|t| t.name == "dev").unwrap();
        assert_eq!(dev_task.command, "cargo watch -x run");
    }

    #[test]
    fn test_parse_default_commands() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("Cargo.toml");
        fs::write(
            &path,
            r#"
[package]
name = "mylib"
version = "0.1.0"
"#,
        )
        .unwrap();

        let parser = CargoTomlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        // Should have default commands
        assert!(runner.tasks.iter().any(|t| t.name == "build"));
        assert!(runner.tasks.iter().any(|t| t.name == "test"));
    }
}
