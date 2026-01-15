# task

A fast CLI tool that discovers and runs tasks from various config files in your project.

Scans your directory for task runner configs (package.json, Makefile, Cargo.toml, etc.) and presents an interactive picker to run them.

![demo](https://github.com/user-attachments/assets/cb07b554-fb1f-4c85-9cc9-e76debe19809)

## Why task?

In monorepos and multi-package projects, running tasks becomes tedious. You need to remember package names, navigate between directories, and keep track of which scripts exist where.

**task** solves this by scanning your entire project and presenting every available task in a single, filterable view. No more `cd apps/web && npm run dev` or trying to remember if it was `pnpm --filter @org/api run build` or `yarn workspace api build`.

Just run `task`, type a few characters to filter, and hit enter.

## Installation

```bash
brew install elob/tap/task-runner-detector   # Homebrew (macOS/Linux)
cargo install task-runner-detector            # Cargo
npm install -g task-runner-detector           # npm
```

<details>
<summary>More installation options</summary>

**Shell (macOS/Linux):**
```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/elob/task-runner-detector/releases/latest/download/task-runner-detector-installer.sh | sh
```

**PowerShell (Windows):**
```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/elob/task-runner-detector/releases/latest/download/task-runner-detector-installer.ps1 | iex"
```

**From source:**
```bash
git clone https://github.com/elob/task-runner-detector && cd task-runner-detector && cargo install --path .
```

</details>

## Usage

```bash
# Interactive mode - scan current directory, pick and run a task
task

# Scan a specific directory
task /path/to/project

# JSON output for scripting
task --json
task --json /path/to/project

# Include files/folders ignored by .gitignore
task --no-ignore
```

### Interactive Mode

The interactive picker shows all discovered tasks organized by folder:

- Type to fuzzy-filter tasks by name, runner, or path
- Use arrow keys to navigate
- Press **Tab** to edit the command before running
- Press **Tab** again to expand to the actual script content (e.g., expand `npm run build` to `tsc && esbuild...`)
- Press **Enter** to run the selected task
- Press **Esc** to cancel

**Readline keybindings in edit mode:**
- `Ctrl+A` / `Ctrl+E` - Jump to start/end
- `Ctrl+W` - Delete previous word
- `Ctrl+U` - Delete line
- `Ctrl+K` - Delete to end

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
