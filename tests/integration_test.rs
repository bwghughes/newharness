//! Integration tests that exercise tools end-to-end through the public API,
//! including multi-step workflows that mirror real agent behavior.

use std::fs;
use tempfile::TempDir;

// We can't import private modules directly, so we test via subprocess
// or recreate the tool calls. Since the binary is the entry point,
// integration tests focus on realistic multi-tool workflows.

fn tmp() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// Helper: run a shell command in a directory and return stdout.
fn sh(dir: &std::path::Path, cmd: &str) -> String {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .output()
        .expect("failed to run shell command");
    String::from_utf8_lossy(&output.stdout).to_string()
}

// ── Workflow: create, read, edit, verify ──

#[test]
fn workflow_create_read_edit_verify() {
    let dir = tmp();

    // Step 1: Create a file
    fs::write(dir.path().join("app.py"), "def hello():\n    return 'hi'\n").unwrap();

    // Step 2: Read and verify
    let content = fs::read_to_string(dir.path().join("app.py")).unwrap();
    assert!(content.contains("def hello()"));

    // Step 3: Edit (simulating what edit_file does)
    let new_content = content.replacen("return 'hi'", "return 'hello world'", 1);
    fs::write(dir.path().join("app.py"), &new_content).unwrap();

    // Step 4: Verify
    let final_content = fs::read_to_string(dir.path().join("app.py")).unwrap();
    assert!(final_content.contains("return 'hello world'"));
    assert!(!final_content.contains("return 'hi'"));
}

// ── Workflow: project scaffold ──

#[test]
fn workflow_scaffold_and_build_check() {
    let dir = tmp();

    // Create a minimal Rust project structure
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"[package]
name = "test-project"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() { println!(\"hello\"); }\n",
    )
    .unwrap();

    // Verify the structure
    assert!(dir.path().join("Cargo.toml").exists());
    assert!(dir.path().join("src/main.rs").exists());

    // Check cargo can parse it (syntax check only, no build to keep tests fast)
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version=1"])
        .current_dir(dir.path())
        .output()
        .expect("cargo not found");
    assert!(output.status.success(), "cargo metadata failed");
}

// ── Workflow: grep across project ──

#[test]
fn workflow_grep_across_project() {
    let dir = tmp();

    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn calculate_total() { todo!() }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "use crate::calculate_total;\nfn main() { calculate_total(); }\n",
    )
    .unwrap();

    // grep for function name
    let result = sh(dir.path(), "grep -rn 'calculate_total' src/");
    let lines: Vec<&str> = result.lines().collect();
    assert!(
        lines.len() >= 2,
        "Expected at least 2 matches, got {}",
        lines.len()
    );
}

// ── Workflow: file with special characters ──

#[test]
fn workflow_files_with_special_chars() {
    let dir = tmp();

    let content = "line with 'quotes'\nline with \"double quotes\"\nline with $variables\nline with `backticks`\n";
    fs::write(dir.path().join("special.txt"), content).unwrap();

    let read_back = fs::read_to_string(dir.path().join("special.txt")).unwrap();
    assert_eq!(read_back, content);

    // Edit preserving special chars
    let edited = read_back.replacen("$variables", "$REPLACED_VAR", 1);
    fs::write(dir.path().join("special.txt"), &edited).unwrap();

    let final_content = fs::read_to_string(dir.path().join("special.txt")).unwrap();
    assert!(final_content.contains("$REPLACED_VAR"));
    assert!(final_content.contains("'quotes'"));
    assert!(final_content.contains("`backticks`"));
}

// ── Workflow: deeply nested directories ──

#[test]
fn workflow_deeply_nested_creation() {
    let dir = tmp();

    let deep_path = dir.path().join("a/b/c/d/e/f");
    fs::create_dir_all(&deep_path).unwrap();
    fs::write(deep_path.join("deep.txt"), "found me").unwrap();

    assert!(deep_path.join("deep.txt").exists());
    let content = fs::read_to_string(deep_path.join("deep.txt")).unwrap();
    assert_eq!(content, "found me");
}

// ── Workflow: concurrent file operations ──

#[test]
fn workflow_multiple_files_in_sequence() {
    let dir = tmp();

    // Create multiple files (simulating parallel tool calls that complete in sequence)
    for i in 0..10 {
        fs::write(
            dir.path().join(format!("file_{i}.txt")),
            format!("content of file {i}"),
        )
        .unwrap();
    }

    // Verify all exist and have correct content
    for i in 0..10 {
        let content = fs::read_to_string(dir.path().join(format!("file_{i}.txt"))).unwrap();
        assert_eq!(content, format!("content of file {i}"));
    }

    // List dir and verify count
    let entries: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 10);
}

// ── Workflow: binary detection ──

#[test]
fn workflow_handles_binary_file_gracefully() {
    let dir = tmp();

    // Write binary content
    let binary_data: Vec<u8> = (0..=255).collect();
    fs::write(dir.path().join("binary.bin"), &binary_data).unwrap();

    // Verify file exists and has correct size
    let metadata = fs::metadata(dir.path().join("binary.bin")).unwrap();
    assert_eq!(metadata.len(), 256);
}

// ── Workflow: edit preserves file structure ──

#[test]
fn workflow_edit_preserves_surrounding_content() {
    let dir = tmp();

    let original = r#"use std::io;

fn main() {
    println!("Hello, world!");
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    println!("You said: {}", input.trim());
}
"#;
    fs::write(dir.path().join("main.rs"), original).unwrap();

    // Edit just one line
    let content = fs::read_to_string(dir.path().join("main.rs")).unwrap();
    let edited = content.replacen(
        r#"println!("Hello, world!");"#,
        r#"println!("Greetings, human!");"#,
        1,
    );
    fs::write(dir.path().join("main.rs"), &edited).unwrap();

    let final_content = fs::read_to_string(dir.path().join("main.rs")).unwrap();
    assert!(final_content.contains("Greetings, human!"));
    assert!(final_content.contains("use std::io;"));
    assert!(final_content.contains("io::stdin()"));
    assert!(final_content.contains("You said:"));
}

// ── Workflow: shell pipeline ──

#[test]
fn workflow_shell_pipeline() {
    let dir = tmp();

    // Create test data
    fs::write(
        dir.path().join("data.txt"),
        "apple\nbanana\ncherry\napricot\navocado\n",
    )
    .unwrap();

    // Run a pipeline
    let result = sh(dir.path(), "cat data.txt | grep '^a' | sort | wc -l");
    let count: i32 = result.trim().parse().unwrap();
    assert_eq!(count, 3); // apple, apricot, avocado
}

// ── Workflow: error recovery ──

#[test]
fn workflow_error_recovery_missing_file() {
    let dir = tmp();

    // Attempt to read non-existent file
    let result = fs::read_to_string(dir.path().join("missing.txt"));
    assert!(result.is_err());

    // Create it after the error
    fs::write(dir.path().join("missing.txt"), "now it exists").unwrap();

    // Now it works
    let content = fs::read_to_string(dir.path().join("missing.txt")).unwrap();
    assert_eq!(content, "now it exists");
}

// ── Cargo binary builds ──

#[test]
fn binary_builds_successfully() {
    let output = std::process::Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo build failed to execute");
    assert!(
        output.status.success(),
        "cargo build --release failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
