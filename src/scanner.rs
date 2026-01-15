//! Directory scanner for task runner config files

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use ignore::{WalkBuilder, WalkState};

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

/// Scan a directory tree for task runners with custom options.
/// Uses scan_streaming internally and collects results.
pub fn scan_with_options(
    root: impl AsRef<Path>,
    options: ScanOptions,
) -> ScanResult<Vec<TaskRunner>> {
    use std::sync::mpsc;

    let root = root.as_ref().to_path_buf();
    let (tx, rx) = mpsc::channel();

    let handle = scan_streaming(root, options, tx);

    // Collect all results
    let runners: Vec<TaskRunner> = rx.into_iter().collect();

    // Wait for scanner to finish
    handle.join().ok();

    Ok(runners)
}

/// Scan a directory tree for task runners, streaming results through a channel.
/// Uses parallel walking for better performance on large directories.
/// Returns a JoinHandle that completes when scanning is done.
pub fn scan_streaming(
    root: PathBuf,
    options: ScanOptions,
    tx: Sender<TaskRunner>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut builder = WalkBuilder::new(&root);
        builder.follow_links(false);
        builder.standard_filters(!options.no_ignore);

        if let Some(max_depth) = options.max_depth {
            builder.max_depth(Some(max_depth));
        }

        builder.build_parallel().run(|| {
            let tx = tx.clone();
            Box::new(move |result| {
                let entry = match result {
                    Ok(e) => e,
                    Err(_) => return WalkState::Continue,
                };

                if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                    return WalkState::Continue;
                }

                let path = entry.path();
                let file_name = match path.file_name() {
                    Some(name) => name.to_string_lossy(),
                    None => return WalkState::Continue,
                };

                let parser: Option<Box<dyn Parser>> = match file_name.as_ref() {
                    "package.json" => Some(Box::new(parsers::PackageJsonParser)),
                    "Makefile" | "makefile" | "GNUmakefile" => {
                        Some(Box::new(parsers::MakefileParser))
                    }
                    "Cargo.toml" => Some(Box::new(parsers::CargoTomlParser)),
                    "pubspec.yaml" => Some(Box::new(parsers::PubspecYamlParser)),
                    "turbo.json" => Some(Box::new(parsers::TurboJsonParser)),
                    "pyproject.toml" => Some(Box::new(parsers::PyprojectTomlParser)),
                    "justfile" | "Justfile" | ".justfile" => {
                        Some(Box::new(parsers::JustfileParser))
                    }
                    "deno.json" | "deno.jsonc" => Some(Box::new(parsers::DenoJsonParser)),
                    "pom.xml" => Some(Box::new(parsers::PomXmlParser)),
                    name if name.ends_with(".csproj")
                        || name.ends_with(".fsproj")
                        || name.ends_with(".vbproj") =>
                    {
                        Some(Box::new(parsers::CsprojParser))
                    }
                    _ => None,
                };

                if let Some(parser) = parser {
                    if let Ok(Some(runner)) = parser.parse(path) {
                        if !runner.tasks.is_empty() && tx.send(runner).is_err() {
                            return WalkState::Quit;
                        }
                    }
                }

                WalkState::Continue
            })
        });
    })
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
