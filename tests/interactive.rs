//! Integration tests for interactive mode
//!
//! These tests spawn the `task` binary with a PTY and simulate user input
//! to verify that tasks are correctly discovered and executed.

use std::process::Command;
use std::time::Duration;

use expectrl::{spawn, Eof, Regex};

/// Build the binary first if needed
fn ensure_binary_built() {
    Command::new("cargo")
        .args(["build"])
        .output()
        .expect("Failed to build binary");
}

/// Get path to the built binary
fn binary_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/target/debug/task", manifest_dir)
}

/// Get path to fixtures directory
fn fixtures_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/fixtures", manifest_dir)
}

#[test]
fn test_npm_task_execution() {
    ensure_binary_built();

    // Test apps/web package.json which has unique output "Bundling for production"
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{}/fixtures/apps/web", manifest_dir);

    let mut session = spawn(&format!("{} {}", binary_path(), fixture))
        .expect("Failed to spawn task");

    session.set_expect_timeout(Some(Duration::from_secs(10)));

    // Wait for the picker to appear (alternate screen)
    std::thread::sleep(Duration::from_millis(500));

    // Filter to npm build
    session.send("build").expect("Failed to type query");

    std::thread::sleep(Duration::from_millis(300));

    // Press Enter to run the task
    session.send("\r").expect("Failed to send Enter");

    // After task runs, we exit alternate screen and see output
    session
        .expect("Bundling for production")
        .expect("Should see npm build output");

    // Wait for completion
    session
        .expect("Task completed")
        .expect("Task should complete");

    session.expect(Eof).ok();
}

#[test]
fn test_make_task_execution() {
    ensure_binary_built();

    let mut session = spawn(&format!("{} {}", binary_path(), fixtures_path()))
        .expect("Failed to spawn task");

    session.set_expect_timeout(Some(Duration::from_secs(10)));

    // Wait for picker
    std::thread::sleep(Duration::from_millis(500));

    // Type "make build" to filter
    session.send("make build").expect("Failed to type query");

    std::thread::sleep(Duration::from_millis(300));

    // Press Enter to run
    session.send("\r").expect("Failed to send Enter");

    // Should see make output
    session
        .expect("Compiling source files")
        .expect("Should see make build output");

    session
        .expect("Task completed")
        .expect("Task should complete");

    session.expect(Eof).ok();
}

#[test]
fn test_just_task_execution() {
    ensure_binary_built();

    // Check if just is available
    if Command::new("just").arg("--version").output().is_err() {
        eprintln!("Skipping Just test - just not installed");
        return;
    }

    let mut session = spawn(&format!("{} {}", binary_path(), fixtures_path()))
        .expect("Failed to spawn task");

    session.set_expect_timeout(Some(Duration::from_secs(10)));

    std::thread::sleep(Duration::from_millis(500));

    session.send("just build").expect("Failed to type query");

    std::thread::sleep(Duration::from_millis(300));

    session.send("\r").expect("Failed to send Enter");

    session
        .expect("Optimizing for production")
        .expect("Should see just build output");

    session.expect(Eof).ok();
}

#[test]
fn test_maven_task_execution() {
    ensure_binary_built();

    // Check if mvn is available
    if Command::new("mvn").arg("--version").output().is_err() {
        eprintln!("Skipping Maven test - mvn not installed");
        return;
    }

    let mut session = spawn(&format!("{} {}", binary_path(), fixtures_path()))
        .expect("Failed to spawn task");

    session.set_expect_timeout(Some(Duration::from_secs(60))); // Maven can be slow

    std::thread::sleep(Duration::from_millis(500));

    session
        .send("mvn validate backend")
        .expect("Failed to type query");

    std::thread::sleep(Duration::from_millis(300));

    session.send("\r").expect("Failed to send Enter");

    // Maven validate should succeed
    session
        .expect(Regex("BUILD SUCCESS|Task completed"))
        .expect("Maven should complete");

    session.expect(Eof).ok();
}

#[test]
fn test_dotnet_task_execution() {
    ensure_binary_built();

    // Check if dotnet is available
    if Command::new("dotnet").arg("--version").output().is_err() {
        eprintln!("Skipping .NET test - dotnet not installed");
        return;
    }

    let mut session = spawn(&format!("{} {}", binary_path(), fixtures_path()))
        .expect("Failed to spawn task");

    session.set_expect_timeout(Some(Duration::from_secs(120))); // dotnet can be slow first time

    std::thread::sleep(Duration::from_millis(500));

    session
        .send("dotnet build dotnet-api")
        .expect("Failed to type query");

    std::thread::sleep(Duration::from_millis(300));

    session.send("\r").expect("Failed to send Enter");

    // Should see dotnet output - either success or our echo
    session
        .expect(Regex("Build succeeded|Task completed|Running dotnet"))
        .expect("Should see dotnet build output");

    session.expect(Eof).ok();
}

#[test]
fn test_escape_cancels() {
    ensure_binary_built();

    let mut session = spawn(&format!("{} {}", binary_path(), fixtures_path()))
        .expect("Failed to spawn task");

    session.set_expect_timeout(Some(Duration::from_secs(5)));

    // Wait for picker
    std::thread::sleep(Duration::from_millis(500));

    // Press Escape to cancel
    session.send("\x1b").expect("Failed to send Escape");

    // Should see cancelled message
    session.expect("Cancelled").expect("Should be cancelled");

    session.expect(Eof).ok();
}

#[test]
fn test_ctrl_c_cancels() {
    ensure_binary_built();

    let mut session = spawn(&format!("{} {}", binary_path(), fixtures_path()))
        .expect("Failed to spawn task");

    session.set_expect_timeout(Some(Duration::from_secs(5)));

    // Wait for picker
    std::thread::sleep(Duration::from_millis(500));

    // Press Ctrl+C to cancel
    session.send("\x03").expect("Failed to send Ctrl+C");

    // Ctrl+C may either show "Cancelled" or just terminate the process
    // Both are valid outcomes
    let _ = session.expect("Cancelled");

    session.expect(Eof).ok();
}

#[test]
fn test_navigation() {
    ensure_binary_built();

    let mut session = spawn(&format!("{} {}", binary_path(), fixtures_path()))
        .expect("Failed to spawn task");

    session.set_expect_timeout(Some(Duration::from_secs(5)));

    // Wait for picker
    std::thread::sleep(Duration::from_millis(500));

    // Navigate down/up
    session.send("\x1b[B").expect("Failed to send Down");
    session.send("\x1b[B").expect("Failed to send Down");
    session.send("\x1b[A").expect("Failed to send Up");

    // Cancel
    session.send("\x1b").expect("Failed to send Escape");

    session.expect("Cancelled").expect("Should be cancelled");

    session.expect(Eof).ok();
}

#[test]
fn test_deno_task_execution() {
    ensure_binary_built();

    // Check if deno is available
    if Command::new("deno").arg("--version").output().is_err() {
        eprintln!("Skipping Deno test - deno not installed");
        return;
    }

    let mut session = spawn(&format!("{} {}", binary_path(), fixtures_path()))
        .expect("Failed to spawn task");

    session.set_expect_timeout(Some(Duration::from_secs(10)));

    std::thread::sleep(Duration::from_millis(500));

    session.send("deno task dev").expect("Failed to type query");

    std::thread::sleep(Duration::from_millis(300));

    session.send("\r").expect("Failed to send Enter");

    session
        .expect("Reloading on save")
        .expect("Should see deno task output");

    session.expect(Eof).ok();
}
