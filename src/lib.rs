//! Task Runner Detector - Discover and run tasks from various config files
//!
//! This crate scans a directory tree and discovers runnable tasks from common
//! task runner configuration files like package.json, Makefile, Cargo.toml, etc.
//!
//! # Example
//!
//! ```no_run
//! use task_runner_detector::scan;
//!
//! let runners = scan(".").unwrap();
//! for runner in runners {
//!     println!("{:?} @ {}", runner.runner_type, runner.config_path.display());
//!     for task in &runner.tasks {
//!         println!("  {} -> {}", task.name, task.command);
//!     }
//! }
//! ```

mod parsers;
mod scanner;

use std::path::PathBuf;
use thiserror::Error;

pub use scanner::{scan, scan_streaming, scan_with_options, ScanOptions};

/// The type of task runner detected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunnerType {
    Npm,
    Bun,
    Yarn,
    Pnpm,
    Make,
    Cargo,
    Flutter,
    Dart,
    Turbo,
    Poetry,
    Pdm,
    Just,
    Deno,
}

impl RunnerType {
    /// Returns a human-readable display name for the runner type
    pub fn display_name(&self) -> &'static str {
        match self {
            RunnerType::Npm => "npm",
            RunnerType::Bun => "bun",
            RunnerType::Yarn => "yarn",
            RunnerType::Pnpm => "pnpm",
            RunnerType::Make => "make",
            RunnerType::Cargo => "cargo",
            RunnerType::Flutter => "flutter",
            RunnerType::Dart => "dart",
            RunnerType::Turbo => "turbo",
            RunnerType::Poetry => "poetry",
            RunnerType::Pdm => "pdm",
            RunnerType::Just => "just",
            RunnerType::Deno => "deno",
        }
    }

    /// Get an icon/emoji for the runner type
    pub fn icon(&self) -> &'static str {
        match self {
            RunnerType::Npm => "📦",
            RunnerType::Bun => "🥟",
            RunnerType::Yarn => "🧶",
            RunnerType::Pnpm => "📦",
            RunnerType::Make => "🔨",
            RunnerType::Cargo => "🦀",
            RunnerType::Flutter => "💙",
            RunnerType::Dart => "🎯",
            RunnerType::Turbo => "⚡",
            RunnerType::Poetry => "🐍",
            RunnerType::Pdm => "🐍",
            RunnerType::Just => "📜",
            RunnerType::Deno => "🦕",
        }
    }

    /// Get a suggested terminal color for this runner type
    pub fn color_code(&self) -> u8 {
        match self {
            RunnerType::Npm => 1,     // Red
            RunnerType::Bun => 3,     // Yellow
            RunnerType::Yarn => 4,    // Blue
            RunnerType::Pnpm => 3,    // Yellow
            RunnerType::Make => 2,    // Green
            RunnerType::Cargo => 1,   // Red
            RunnerType::Flutter => 6, // Cyan
            RunnerType::Dart => 6,    // Cyan
            RunnerType::Turbo => 5,   // Magenta
            RunnerType::Poetry => 2,  // Green
            RunnerType::Pdm => 2,     // Green
            RunnerType::Just => 3,    // Yellow
            RunnerType::Deno => 2,    // Green
        }
    }
}

impl std::fmt::Display for RunnerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// A single task that can be run
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Task {
    /// The name of the task (e.g., "build", "test", "dev")
    pub name: String,
    /// The full command to run (e.g., "npm run build", "make test")
    pub command: String,
    /// Optional description of what the task does
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The actual script content (e.g., the shell command in package.json scripts)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

/// A task runner configuration file with its discovered tasks
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskRunner {
    /// Path to the config file (e.g., "apps/mobile/pubspec.yaml")
    pub config_path: PathBuf,
    /// The type of task runner
    pub runner_type: RunnerType,
    /// List of tasks discovered in the config file
    pub tasks: Vec<Task>,
}

/// Errors that can occur during scanning
#[derive(Error, Debug)]
pub enum ScanError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse {path}: {message}")]
    ParseError { path: PathBuf, message: String },

    #[error("Walk error: {0}")]
    WalkError(#[from] ignore::Error),
}

/// Result type for scan operations
pub type ScanResult<T> = Result<T, ScanError>;
