//! Directory scanner for task runner config files

use std::path::Path;

use ignore::WalkBuilder;

use crate::parsers::{self, Parser};
use crate::{ScanResult, TaskRunner};

/// Options for customizing the scan behavior
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Maximum depth to traverse (None = unlimited)
    pub max_depth: Option<usize>,
    /// If true, ignore .gitignore and scan all files
    pub no_ignore: bool,
}

/// Scan a directory tree for task runners using default options
pub fn scan(root: impl AsRef<Path>) -> ScanResult<Vec<TaskRunner>> {
    scan_with_options(root, ScanOptions::default())
}

/// Scan a directory tree for task runners with custom options
pub fn scan_with_options(root: impl AsRef<Path>, options: ScanOptions) -> ScanResult<Vec<TaskRunner>> {
    let root = root.as_ref();
    let mut runners = Vec::new();

    let mut builder = WalkBuilder::new(root);
    builder.follow_links(false);
    builder.standard_filters(!options.no_ignore);

    if let Some(max_depth) = options.max_depth {
        builder.max_depth(Some(max_depth));
    }

    for result in builder.build() {
        let entry = result?;

        // Only process files
        let is_file = entry.file_type().map(|ft| ft.is_file()).unwrap_or(false);
        if !is_file {
            continue;
        }

        let path = entry.path();
        let file_name = match path.file_name() {
            Some(name) => name.to_string_lossy(),
            None => continue,
        };

        // Match config files and parse them
        let parser: Option<Box<dyn Parser>> = match file_name.as_ref() {
            "package.json" => Some(Box::new(parsers::PackageJsonParser)),
            "Makefile" | "makefile" | "GNUmakefile" => Some(Box::new(parsers::MakefileParser)),
            "Cargo.toml" => Some(Box::new(parsers::CargoTomlParser)),
            "pubspec.yaml" => Some(Box::new(parsers::PubspecYamlParser)),
            "turbo.json" => Some(Box::new(parsers::TurboJsonParser)),
            "pyproject.toml" => Some(Box::new(parsers::PyprojectTomlParser)),
            "justfile" | "Justfile" | ".justfile" => Some(Box::new(parsers::JustfileParser)),
            "deno.json" | "deno.jsonc" => Some(Box::new(parsers::DenoJsonParser)),
            _ => None,
        };

        if let Some(parser) = parser {
            match parser.parse(path) {
                Ok(Some(runner)) => {
                    // Only add if there are tasks
                    if !runner.tasks.is_empty() {
                        runners.push(runner);
                    }
                }
                Ok(None) => {
                    // Parser decided this file doesn't have relevant tasks
                }
                Err(e) => {
                    // Log but don't fail on parse errors - continue scanning
                    eprintln!("Warning: Failed to parse {}: {}", path.display(), e);
                }
            }
        }
    }

    Ok(runners)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scan_empty_dir() {
        let dir = TempDir::new().unwrap();
        let runners = scan(dir.path()).unwrap();
        assert!(runners.is_empty());
    }

    #[test]
    fn test_scan_respects_gitignore() {
        use std::process::Command;

        let dir = TempDir::new().unwrap();

        // Initialize a git repo (required for .gitignore to work)
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .ok();

        // Create a .gitignore that ignores the ignored/ directory
        fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();

        // Create a package.json in ignored/ (should be ignored)
        let ignored_dir = dir.path().join("ignored");
        fs::create_dir_all(&ignored_dir).unwrap();
        fs::write(
            ignored_dir.join("package.json"),
            r#"{"scripts": {"test": "echo test"}}"#,
        )
        .unwrap();

        // Create a package.json at root (should be found)
        fs::write(
            dir.path().join("package.json"),
            r#"{"scripts": {"build": "echo build"}}"#,
        )
        .unwrap();

        let runners = scan(dir.path()).unwrap();
        assert_eq!(runners.len(), 1);
        assert!(runners[0]
            .config_path
            .to_string_lossy()
            .contains("package.json"));
    }

    #[test]
    fn test_scan_no_ignore() {
        let dir = TempDir::new().unwrap();

        // Create a .gitignore that ignores the ignored/ directory
        fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();

        // Create a package.json in ignored/
        let ignored_dir = dir.path().join("ignored");
        fs::create_dir_all(&ignored_dir).unwrap();
        fs::write(
            ignored_dir.join("package.json"),
            r#"{"scripts": {"test": "echo test"}}"#,
        )
        .unwrap();

        // Create a package.json at root
        fs::write(
            dir.path().join("package.json"),
            r#"{"scripts": {"build": "echo build"}}"#,
        )
        .unwrap();

        // With no_ignore, should find both
        let options = ScanOptions {
            no_ignore: true,
            ..Default::default()
        };
        let runners = scan_with_options(dir.path(), options).unwrap();
        assert_eq!(runners.len(), 2);
    }
}
