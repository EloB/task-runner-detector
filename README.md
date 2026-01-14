# task

A fast CLI tool that discovers and runs tasks from various config files in your project.

Scans your directory for task runner configs (package.json, Makefile, Cargo.toml, etc.) and presents an interactive picker to run them.

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Interactive mode - scan current directory, pick and run a task
task

# Scan a specific directory
task /path/to/project

# JSON output for scripting
task --json
task --json /path/to/project
```

### Interactive Mode

The interactive picker shows all discovered tasks organized by folder:

- Type to filter tasks by name, runner, or folder
- Use arrow keys to navigate
- Press Enter to run the selected task
- Press Esc to cancel

## Supported Task Runners

| Runner | Config File | Tasks |
|--------|-------------|-------|
| npm/yarn/pnpm/bun | `package.json` | Scripts from `scripts` field |
| Make | `Makefile` | Makefile targets |
| Cargo | `Cargo.toml` | Binary targets, `[package.metadata.scripts]` |
| Turbo | `turbo.json` | Pipeline tasks |
| Just | `justfile` | Just recipes |
| Deno | `deno.json` | Deno tasks |
| Poetry | `pyproject.toml` | Poetry scripts |
| PDM | `pyproject.toml` | PDM scripts |
| Flutter/Dart | `pubspec.yaml` | Custom scripts |

## Library Usage

```rust
use task_runner_detector::scan;

fn main() {
    let runners = scan(".").unwrap();
    for runner in runners {
        println!("{} @ {}", runner.runner_type, runner.config_path.display());
        for task in &runner.tasks {
            println!("  {} -> {}", task.name, task.command);
        }
    }
}
```

## License

MIT
