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
cargo run -- -s                      # Streaming NDJSON output
cargo run -- -j -q "build"           # Filter with fuzzy search
cargo run -- -i                      # Include gitignored files

# Run tests
cargo test

# Run only integration tests
cargo test --test interactive

# Run a specific test
cargo test test_parse_npm_scripts

# Check for errors without building
cargo check

# Format code
cargo fmt

# Lint with clippy
cargo clippy -- -D warnings
```

## Architecture

This is a Rust CLI tool that scans directories for task runner configuration files and presents an interactive picker to run tasks.

### Core Structure

- **`src/main.rs`**: CLI entry point (~1500 lines) containing:
  - Clap argument parsing (`-j/--json`, `-s/--json-stream`, `-q/--query`, `-i/--no-ignore`)
  - Interactive TUI with three modes (Select, Edit, Expanded)
  - Fuzzy search engine using `nucleo-matcher` (fzf-compatible syntax)
  - Tree-based UI rendering with folder hierarchy
  - Task execution with working directory handling

- **`src/lib.rs`**: Public library API exposing:
  - `scan()`, `scan_with_options()`, `scan_streaming()` functions
  - Core types: `Task`, `TaskRunner`, `RunnerType`, `ScanOptions`
  - Error types: `ScanError`, `ScanResult`

- **`src/scanner.rs`**: Parallel directory walker using the `ignore` crate:
  - Respects `.gitignore` by default
  - Dispatches files to appropriate parsers by filename
  - Streams results via channels for real-time UI updates

- **`src/parsers/`**: Parser implementations (each ~100-220 lines):
  - `package_json.rs` - npm/bun/yarn/pnpm scripts (detects from `packageManager` field)
  - `cargo_toml.rs` - Cargo binaries and `[package.metadata.scripts]`
  - `makefile.rs` - Makefile targets (line-based parsing, skips patterns/variables)
  - `turbo_json.rs` - Turborepo v1 (`pipeline`) and v2 (`tasks`) formats
  - `pyproject_toml.rs` - Poetry, PDM, and PEP 621 scripts
  - `pubspec_yaml.rs` - Flutter/Dart scripts
  - `justfile.rs` - Just recipes (uses `just` crate's summary API)
  - `deno_json.rs` - Deno tasks (supports `.jsonc` with comments)
  - `pom_xml.rs` - Maven lifecycle phases, profiles, and plugin goals
  - `csproj.rs` - .NET CLI commands and custom MSBuild targets

- **`tests/interactive.rs`**: Integration tests using `expectrl` for PTY simulation

- **`fixtures/`**: Test monorepo with various config formats

### Interactive UI Architecture

The TUI uses a functional architecture with normalized state:

```rust
struct AppState {
    tasks_by_id: HashMap<TaskId, DisplayTask>,  // O(1) task lookups
    task_ids: Vec<TaskId>,                       // Insertion order
    folder_ids: Vec<String>,                     // Folder discovery order
    query: String,                               // Search input
    selected_task: Option<TaskId>,               // Current selection
    mode: Mode,                                  // Select | Edit | Expanded
    edit_buffer: String,                         // Command being edited
    scroll_offset: usize,                        // Viewport position
}
```

**Three Interaction Modes:**
1. **Select** - Type to fuzzy search, navigate with arrows, Tab to edit
2. **Edit** - Modify command before running, Tab to expand script
3. **Expanded** - View/edit full script content, Esc to go back

**Rendering Pipeline:**
1. `derive_items()` - Build tree from normalized state
2. `compute_matching_tasks()` - Fuzzy match with highlight indices
3. `derive_filtered()` - Filter keeping tree structure
4. `render()` - Generate ANSI output with colors and icons

### Parser Pattern

All parsers implement the `Parser` trait:
```rust
pub trait Parser {
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError>;
}
```

Return `Ok(None)` if the file has no relevant tasks, `Ok(Some(TaskRunner))` on success, or `Err` for parse failures. Parsers are stateless unit structs for thread safety.

### Key Types

```rust
pub enum RunnerType {
    Npm, Bun, Yarn, Pnpm,  // Node.js
    Make, Cargo,            // Build systems
    Flutter, Dart,          // Dart ecosystem
    Turbo,                  // Monorepo
    Poetry, Pdm,            // Python
    Just, Deno,             // Task runners
    Maven, DotNet,          // Java/.NET
}

pub struct Task {
    pub name: String,
    pub command: String,               // e.g., "npm run build"
    pub description: Option<String>,
    pub script: Option<String>,        // Actual script content for expansion
}

pub struct TaskRunner {
    pub config_path: PathBuf,
    pub runner_type: RunnerType,
    pub tasks: Vec<Task>,
}
```

### Streaming Architecture

Scanning uses parallel directory walking with streaming results:

```rust
// Scanner runs in separate thread, sends results via channel
let (tx, rx) = mpsc::channel();
scan_streaming(root, options, tx);  // Returns JoinHandle

// Main loop receives tasks while rendering UI
for runner in rx.try_recv() {
    // Update UI incrementally
}
```

### Tests

- **Unit tests**: In each parser module (`cargo test`)
- **Integration tests**: `tests/interactive.rs` spawns binary with PTY simulation
  - Tests task execution for npm, make, maven, dotnet, deno, just
  - Tests navigation, cancellation (Esc, Ctrl+C)
  - Conditional tests skip if CLI tools not installed

### Key Dependencies

- `ignore` - .gitignore-respecting parallel directory walker
- `nucleo-matcher` - Fuzzy matching (fzf syntax)
- `crossterm` - Terminal UI (raw mode, colors, cursor)
- `quick-xml` - XML parsing (Maven, .NET)
- `serde` + `serde_json`/`toml`/`serde-saphyr` - Config parsing
- `just` - Justfile parsing via crate API
- `expectrl` - PTY-based integration testing
