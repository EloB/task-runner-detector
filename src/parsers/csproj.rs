//! Parser for MSBuild .csproj/.fsproj/.vbproj files

use std::fs;
use std::path::Path;

use quick_xml::de::from_str;
use serde::Deserialize;

use super::Parser;
use crate::{RunnerType, ScanError, Task, TaskRunner};

/// Standard dotnet CLI commands available for all projects
const STANDARD_COMMANDS: &[(&str, &str)] = &[
    ("build", "Build the project"),
    ("run", "Run the project"),
    ("test", "Run unit tests"),
    ("publish", "Publish the project for deployment"),
    ("clean", "Clean build outputs"),
    ("restore", "Restore NuGet packages"),
    ("pack", "Create a NuGet package"),
];

#[derive(Debug, Deserialize, Default)]
#[serde(rename = "Project")]
struct Project {
    #[serde(rename = "Target", default)]
    targets: Vec<Target>,
    #[serde(rename = "ItemGroup", default)]
    item_groups: Vec<ItemGroup>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Target {
    #[serde(rename = "@Name")]
    name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ItemGroup {
    #[serde(rename = "PackageReference", default)]
    package_references: Vec<PackageReference>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct PackageReference {
    #[serde(rename = "@Include")]
    include: Option<String>,
}

pub struct CsprojParser;

impl CsprojParser {
    /// Check if project has test framework references
    fn has_test_framework(project: &Project) -> bool {
        for item_group in &project.item_groups {
            for pkg in &item_group.package_references {
                if let Some(include) = &pkg.include {
                    let lower = include.to_lowercase();
                    if lower.contains("xunit")
                        || lower.contains("nunit")
                        || lower.contains("mstest")
                        || lower.contains("test")
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
}

impl Parser for CsprojParser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError> {
        let content = fs::read_to_string(path)?;

        let project: Project = from_str(&content).map_err(|e| ScanError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let mut tasks: Vec<Task> = Vec::new();
        let has_tests = Self::has_test_framework(&project);

        // Add standard commands
        for (cmd, description) in STANDARD_COMMANDS {
            // Skip test command if no test framework detected
            if *cmd == "test" && !has_tests {
                continue;
            }
            tasks.push(Task {
                name: cmd.to_string(),
                command: format!("dotnet {}", cmd),
                description: Some(description.to_string()),
                script: None,
            });
        }

        // Add custom MSBuild targets
        for target in &project.targets {
            if let Some(name) = &target.name {
                // Skip common built-in targets
                if matches!(
                    name.as_str(),
                    "Build"
                        | "Clean"
                        | "Rebuild"
                        | "Restore"
                        | "Publish"
                        | "Pack"
                        | "BeforeBuild"
                        | "AfterBuild"
                ) {
                    continue;
                }
                tasks.push(Task {
                    name: format!("msbuild:{}", name),
                    command: format!("dotnet msbuild -t:{}", name),
                    description: Some(format!("Run MSBuild target '{}'", name)),
                    script: None,
                });
            }
        }

        if tasks.is_empty() {
            return Ok(None);
        }

        Ok(Some(TaskRunner {
            config_path: path.to_path_buf(),
            runner_type: RunnerType::DotNet,
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
    fn test_parse_basic_csproj() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("MyApp.csproj");
        fs::write(
            &path,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net8.0</TargetFramework>
  </PropertyGroup>
</Project>"#,
        )
        .unwrap();

        let parser = CsprojParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert_eq!(runner.runner_type, RunnerType::DotNet);
        assert!(runner.tasks.iter().any(|t| t.name == "build"));
        assert!(runner.tasks.iter().any(|t| t.name == "run"));
        // No test framework, so no test command
        assert!(!runner.tasks.iter().any(|t| t.name == "test"));
    }

    #[test]
    fn test_parse_test_project() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("MyApp.Tests.csproj");
        fs::write(
            &path,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net8.0</TargetFramework>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="xunit" Version="2.6.1" />
    <PackageReference Include="xunit.runner.visualstudio" Version="2.5.3" />
  </ItemGroup>
</Project>"#,
        )
        .unwrap();

        let parser = CsprojParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        // Test project should have test command
        assert!(runner.tasks.iter().any(|t| t.name == "test"));
    }

    #[test]
    fn test_parse_custom_targets() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("MyApp.csproj");
        fs::write(
            &path,
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net8.0</TargetFramework>
  </PropertyGroup>
  <Target Name="GenerateCode">
    <Message Text="Generating code..." />
  </Target>
  <Target Name="Deploy">
    <Message Text="Deploying..." />
  </Target>
</Project>"#,
        )
        .unwrap();

        let parser = CsprojParser;
        let runner = parser.parse(&path).unwrap().unwrap();

        assert!(runner
            .tasks
            .iter()
            .any(|t| t.name == "msbuild:GenerateCode"));
        assert!(runner.tasks.iter().any(|t| t.name == "msbuild:Deploy"));
    }
}
