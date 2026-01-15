//! Build script that auto-installs git hooks
//!
//! Hooks are defined in `.hooks/` directory and installed to `.git/hooks/`
//! on first `cargo build`. Skipped in CI environments.

fn main() {
    // Skip in CI
    if std::env::var("CI").is_ok() {
        return;
    }

    // Skip if not in a git repo
    let git_hooks_dir = std::path::Path::new(".git/hooks");
    if !git_hooks_dir.exists() {
        return;
    }

    // Install hooks from .hooks/ directory
    let hooks_dir = std::path::Path::new(".hooks");
    if let Ok(entries) = std::fs::read_dir(hooks_dir) {
        for entry in entries.flatten() {
            let source = entry.path();
            let filename = entry.file_name();
            let dest = git_hooks_dir.join(&filename);

            // Only install if destination doesn't exist
            if !dest.exists() {
                if let Ok(content) = std::fs::read_to_string(&source) {
                    if std::fs::write(&dest, &content).is_ok() {
                        // Make executable on Unix
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ = std::fs::set_permissions(
                                &dest,
                                std::fs::Permissions::from_mode(0o755),
                            );
                        }
                        println!(
                            "cargo:warning=Installed git hook: {}",
                            filename.to_string_lossy()
                        );
                    }
                }
            }
        }
    }
}
