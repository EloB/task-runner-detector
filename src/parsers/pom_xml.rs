//! Parser for Maven pom.xml files

use std::fs;
use std::path::Path;

use quick_xml::de::from_str;
use serde::Deserialize;

use super::Parser;
use crate::{RunnerType, ScanError, Task, TaskRunner};

/// Standard Maven lifecycle phases that are always available
const LIFECYCLE_PHASES: &[(&str, &str)] = &[
    ("validate", "Validate the project is correct"),
    ("compile", "Compile the source code"),
    ("test", "Run unit tests"),
    ("package", "Package compiled code (e.g., JAR)"),
    ("verify", "Run integration tests"),
    ("install", "Install package to local repository"),
    ("deploy", "Deploy package to remote repository"),
    ("clean", "Clean build outputs"),
];

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Project {
    build: Option<Build>,
    profiles: Option<Profiles>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Build {
    plugins: Option<Plugins>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Plugins {
    plugin: Vec<Plugin>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Plugin {
    #[serde(rename = "artifactId")]
    artifact_id: Option<String>,
    executions: Option<Executions>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Executions {
    execution: Vec<Execution>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Execution {
    id: Option<String>,
    goals: Option<Goals>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Goals {
    goal: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Profiles {
    profile: Vec<Profile>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Profile {
    id: Option<String>,
}

pub struct PomXmlParser;

impl Parser for PomXmlParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;

        let project: Project = from_str(&content).map_err(|e| ScanError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let mut tasks: Vec<Task> = Vec::new();

        // Add standard lifecycle phases
        for (phase, description) in LIFECYCLE_PHASES {
            tasks.push(Task {
                name: phase.to_string(),
                command: format!("mvn {}", phase),
                description: Some(description.to_string()),
                script: None,
            });
        }

        // Add profile-specific tasks
        if let Some(profiles) = project.profiles {
            for profile in profiles.profile {
                if let Some(id) = profile.id {
                    tasks.push(Task {
                        name: format!("package -P{}", id),
                        command: format!("mvn package -P{}", id),
                        description: Some(format!("Package with '{}' profile", id)),
                        script: None,
                    });
                }
            }
        }

        // Add plugin goals
        if let Some(build) = project.build {
            if let Some(plugins) = build.plugins {
                for plugin in plugins.plugin {
                    let plugin_name = plugin.artifact_id.unwrap_or_default();
                    if let Some(executions) = plugin.executions {
                        for execution in executions.execution {
                            if let Some(goals) = execution.goals {
                                for goal in goals.goal {
                                    let exec_id = execution.id.clone().unwrap_or_default();
                                    let task_name = if exec_id.is_empty() {
                                        format!("{}:{}", plugin_name, goal)
                                    } else {
                                        format!("{}:{}@{}", plugin_name, goal, exec_id)
                                    };
                                    tasks.push(Task {
                                        name: task_name.clone(),
                                        command: format!("mvn {}", task_name),
                                        description: Some(format!(
                                            "Run {} goal from {}",
                                            goal, plugin_name
                                        )),
                                        script: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        if tasks.is_empty() {
            return Ok(None);
        }

        Ok(Some(TaskRunner {
            config_path: path.to_path_buf(),
            runner_type: RunnerType::Maven,
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
    fn test_parse_basic_pom() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pom.xml");
        fs::write(
            &path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>my-app</artifactId>
    <version>1.0-SNAPSHOT</version>
</project>"#,
        )
        .unwrap();

        let parser = PomXmlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Maven);
        // Should have lifecycle phases
        assert!(runner.tasks.iter().any(|t| t.name == "compile"));
        assert!(runner.tasks.iter().any(|t| t.name == "test"));
        assert!(runner.tasks.iter().any(|t| t.name == "package"));
    }

    #[test]
    fn test_parse_pom_with_profiles() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pom.xml");
        fs::write(
            &path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>my-app</artifactId>
    <version>1.0-SNAPSHOT</version>
    <profiles>
        <profile>
            <id>dev</id>
        </profile>
        <profile>
            <id>prod</id>
        </profile>
    </profiles>
</project>"#,
        )
        .unwrap();

        let parser = PomXmlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert!(runner.tasks.iter().any(|t| t.name == "package -Pdev"));
        assert!(runner.tasks.iter().any(|t| t.name == "package -Pprod"));
    }
}
