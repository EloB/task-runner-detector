# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build and Development Commands

```bash
# Build the project
cargo build

# Run the CLI
cargo run

# Run with arguments
cargo run -- --json /path/to/scan
cargo run -- /path/to/scan

# Run tests
cargo test

# Run a specific test
cargo test test_parse_npm_scripts

# Check for errors without building
cargo check

# Format code
cargo fmt

# Lint with clippy
cargo clippy
```

## Architecture

This is a Rust CLI tool that scans directories for task runner configuration files and presents an interactive picker to run tasks.

### Core Structure

- **`src/main.rs`**: CLI entry point with clap argument parsing. Provides two modes:
  - JSON output mode (`--json`) for programmatic use
  - Interactive terminal picker using crossterm for TUI

- **`src/lib.rs`**: Public library API exposing `scan()`, `scan_with_options()`, core types (`Task`, `TaskRunner`, `RunnerType`), and error types

- **`src/scanner.rs`**: Directory walker using walkdir that discovers config files and dispatches to appropriate parsers. Skips common build/dependency directories (node_modules, target, .git, etc.)

- **`src/parsers/`**: Parser implementations for each supported config format:
  - `package_json.rs` - npm/bun/yarn/pnpm scripts
  - `cargo_toml.rs` - Cargo binaries and [package.metadata.scripts]
  - `makefile.rs` - Makefile targets
  - `turbo_json.rs` - Turborepo pipeline tasks
  - `pyproject_toml.rs` - Poetry/PDM scripts
  - `pubspec_yaml.rs` - Flutter/Dart scripts
  - `justfile.rs` - Just command runner
  - `deno_json.rs` - Deno tasks
  - `pom_xml.rs` - Maven lifecycle phases and profiles
  - `csproj.rs` - .NET CLI commands and MSBuild targets

### Parser Pattern

All parsers implement the `Parser` trait:
```rust
pub trait Parser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError>;
}
```

Return `Ok(None)` if the file has no relevant tasks, `Ok(Some(TaskRunner))` on success, or `Err` for parse failures.

### Key Types

- `RunnerType`: Enum for each supported task runner (Npm, Bun, Cargo, Make, Maven, DotNet, etc.)
- `Task`: Contains `name`, `command`, and optional `description`
- `TaskRunner`: Groups tasks with their `config_path` and `runner_type`

### Tests

- **Unit tests**: In each parser module, run with `cargo test`
- **Integration tests**: `tests/interactive.rs` uses PTY simulation to test the interactive picker
