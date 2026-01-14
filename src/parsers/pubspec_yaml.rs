//! Parser for pubspec.yaml (Flutter/Dart projects)

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::de::IgnoredAny;
use serde::Deserialize;

use crate::{RunnerType, ScanError, Task, TaskRunner};

use super::Parser;

/// We only care about the presence of keys, not their values
/// Using IgnoredAny allows any YAML value without deserializing it
#[derive(Deserialize)]
struct PubspecYaml {
    name: Option<String>,
    #[serde(default)]
    dependencies: HashMap<String, IgnoredAny>,
    #[serde(default)]
    dev_dependencies: HashMap<String, IgnoredAny>,
    #[serde(default)]
    executables: HashMap<String, String>,
    #[serde(default)]
    scripts: HashMap<String, String>, // For derry or similar
}

pub struct PubspecYamlParser;

impl PubspecYamlParser {
    /// Check if this is a Flutter project by looking for flutter dependency
    fn is_flutter_project(pubspec: &PubspecYaml) -> bool {
        pubspec.dependencies.contains_key("flutter")
    }
}

impl Parser for PubspecYamlParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;

        let pubspec: PubspecYaml =
            serde_saphyr::from_str(&content).map_err(|e| ScanError::ParseError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        let is_flutter = Self::is_flutter_project(&pubspec);
        let runner_type = if is_flutter {
            RunnerType::Flutter
        } else {
            RunnerType::Dart
        };

        let mut tasks = Vec::new();

        // Check for derry/custom scripts
        for (name, command) in &pubspec.scripts {
            tasks.push(Task {
                name: name.clone(),
                command: format!("derry {}", name),
                description: Some(command.clone()),
                script: Some(command.clone()),
            });
        }

        // Check for executables (Dart CLI tools)
        for name in pubspec.executables.keys() {
            tasks.push(Task {
                name: name.clone(),
                command: format!("dart run {}", name),
                description: Some(format!("Run the {} executable", name)),
                script: None,
            });
        }

        // Add default commands based on project type
        if is_flutter {
            // Check for build_runner in dev_dependencies
            let has_build_runner = pubspec.dev_dependencies.contains_key("build_runner");

            tasks.push(Task {
                name: "run".to_string(),
                command: "flutter run".to_string(),
                description: Some("Run the Flutter app".to_string()),
                script: None,
            });
            tasks.push(Task {
                name: "test".to_string(),
                command: "flutter test".to_string(),
                description: Some("Run Flutter tests".to_string()),
                script: None,
            });
            tasks.push(Task {
                name: "build-apk".to_string(),
                command: "flutter build apk".to_string(),
                description: Some("Build Android APK".to_string()),
                script: None,
            });
            tasks.push(Task {
                name: "build-ios".to_string(),
                command: "flutter build ios".to_string(),
                description: Some("Build iOS app".to_string()),
                script: None,
            });
            tasks.push(Task {
                name: "analyze".to_string(),
                command: "flutter analyze".to_string(),
                description: Some("Analyze Dart code".to_string()),
                script: None,
            });

            if has_build_runner {
                tasks.push(Task {
                    name: "build_runner".to_string(),
                    command: "dart run build_runner build".to_string(),
                    description: Some("Run code generation".to_string()),
                    script: None,
                });
                tasks.push(Task {
                    name: "build_runner-watch".to_string(),
                    command: "dart run build_runner watch".to_string(),
                    description: Some("Watch and regenerate code".to_string()),
                    script: None,
                });
            }
        } else if pubspec.name.is_some() {
            // Pure Dart project
            tasks.push(Task {
                name: "run".to_string(),
                command: "dart run".to_string(),
                description: Some("Run the Dart app".to_string()),
                script: None,
            });
            tasks.push(Task {
                name: "test".to_string(),
                command: "dart test".to_string(),
                description: Some("Run Dart tests".to_string()),
                script: None,
            });
            tasks.push(Task {
                name: "analyze".to_string(),
                command: "dart analyze".to_string(),
                description: Some("Analyze Dart code".to_string()),
                script: None,
            });
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
    fn test_parse_flutter_project() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pubspec.yaml");
        fs::write(
            &path,
            r#"
name: my_flutter_app
dependencies:
  flutter:
    sdk: flutter
dev_dependencies:
  build_runner: ^2.0.0
"#,
        )
        .unwrap();

        let parser = PubspecYamlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Flutter);
        assert!(runner.tasks.iter().any(|t| t.name == "run"));
        assert!(runner.tasks.iter().any(|t| t.name == "build_runner"));
    }

    #[test]
    fn test_parse_dart_project() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pubspec.yaml");
        fs::write(
            &path,
            r#"
name: my_dart_cli
executables:
  mycli: main
"#,
        )
        .unwrap();

        let parser = PubspecYamlParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::Dart);
        assert!(runner.tasks.iter().any(|t| t.name == "mycli"));
    }
}
