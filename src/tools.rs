use crate::types::*;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Returns the tool definitions array sent to the model.
pub fn tool_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "read_file".into(),
                description: "Read the contents of a file. Returns the file text with line numbers.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to read"
                        }
                    },
                    "required": ["path"]
                }),
            },
        },
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "list_dir".into(),
                description: "List files and directories at the given path. Returns names with trailing / for directories.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path to list (default: current directory)"
                        }
                    },
                    "required": []
                }),
            },
        },
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "edit_file".into(),
                description: "Edit a file by replacing an exact string match with new content, or create a new file. To create a new file, set old_string to empty string.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file"
                        },
                        "old_string": {
                            "type": "string",
                            "description": "Exact string to find and replace. Empty string to create a new file."
                        },
                        "new_string": {
                            "type": "string",
                            "description": "Replacement string"
                        }
                    },
                    "required": ["path", "old_string", "new_string"]
                }),
            },
        },
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "grep".into(),
                description: "Search for a pattern in files recursively. Returns matching lines with file paths and line numbers.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Search pattern (substring match, case-insensitive)"
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search in (default: current directory)"
                        },
                        "glob": {
                            "type": "string",
                            "description": "File glob filter, e.g. '*.rs' (optional)"
                        }
                    },
                    "required": ["pattern"]
                }),
            },
        },
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "bash".into(),
                description: "Run a shell command and return stdout + stderr. Use for build, test, git, and other commands.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The shell command to execute"
                        }
                    },
                    "required": ["command"]
                }),
            },
        },
    ]
}

/// Execute a tool call and return the result string.
pub async fn execute(call: &ToolCall, workdir: &Path) -> String {
    let args: serde_json::Value = match serde_json::from_str(&call.function.arguments) {
        Ok(v) => v,
        Err(e) => return format!("Error parsing arguments: {e}"),
    };

    match call.function.name.as_str() {
        "read_file" => exec_read_file(&args, workdir).await,
        "list_dir" => exec_list_dir(&args, workdir).await,
        "edit_file" => exec_edit_file(&args, workdir).await,
        "grep" => exec_grep(&args, workdir).await,
        "bash" => exec_bash(&args, workdir).await,
        other => format!("Unknown tool: {other}"),
    }
}

fn resolve_path(raw: &str, workdir: &Path) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workdir.join(p)
    }
}

async fn exec_read_file(args: &serde_json::Value, workdir: &Path) -> String {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => resolve_path(p, workdir),
        None => return "Error: missing 'path' parameter".into(),
    };

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            let mut out = String::with_capacity(content.len() + content.lines().count() * 8);
            for (i, line) in content.lines().enumerate() {
                out.push_str(&format!("{:>4}\t{}\n", i + 1, line));
            }
            // Truncate very large files
            if out.len() > 100_000 {
                out.truncate(100_000);
                out.push_str("\n... [truncated at 100KB]");
            }
            out
        }
        Err(e) => format!("Error reading {}: {e}", path.display()),
    }
}

async fn exec_list_dir(args: &serde_json::Value, workdir: &Path) -> String {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_path(p, workdir))
        .unwrap_or_else(|| workdir.to_path_buf());

    let mut entries = match tokio::fs::read_dir(&path).await {
        Ok(rd) => rd,
        Err(e) => return format!("Error listing {}: {e}", path.display()),
    };

    let mut names: Vec<String> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files for cleaner output
        if name.starts_with('.') {
            continue;
        }
        let suffix = if entry
            .file_type()
            .await
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
        {
            "/"
        } else {
            ""
        };
        names.push(format!("{name}{suffix}"));
    }
    names.sort();
    names.join("\n")
}

async fn exec_edit_file(args: &serde_json::Value, workdir: &Path) -> String {
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => resolve_path(p, workdir),
        None => return "Error: missing 'path' parameter".into(),
    };
    let old_string = args
        .get("old_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new_string = args
        .get("new_string")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if old_string.is_empty() {
        // Create new file
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        return match tokio::fs::write(&path, new_string).await {
            Ok(()) => format!("Created {}", path.display()),
            Err(e) => format!("Error creating {}: {e}", path.display()),
        };
    }

    // Read, replace, write
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading {}: {e}", path.display()),
    };

    let count = content.matches(old_string).count();
    if count == 0 {
        return format!(
            "Error: old_string not found in {}. Make sure it matches exactly.",
            path.display()
        );
    }
    if count > 1 {
        return format!(
            "Error: old_string found {count} times in {}. Provide more context for a unique match.",
            path.display()
        );
    }

    let new_content = content.replacen(old_string, new_string, 1);
    match tokio::fs::write(&path, &new_content).await {
        Ok(()) => format!("Edited {} ({} bytes written)", path.display(), new_content.len()),
        Err(e) => format!("Error writing {}: {e}", path.display()),
    }
}

async fn exec_grep(args: &serde_json::Value, workdir: &Path) -> String {
    let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing 'pattern' parameter".into(),
    };

    let search_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_path(p, workdir))
        .unwrap_or_else(|| workdir.to_path_buf());

    let mut cmd_args = vec![
        "-r".to_string(),
        "-n".to_string(),
        "-i".to_string(),
        "--include".to_string(),
    ];

    let glob_pattern = args
        .get("glob")
        .and_then(|v| v.as_str())
        .unwrap_or("*");
    cmd_args.push(glob_pattern.to_string());

    cmd_args.push("--".to_string());
    cmd_args.push(pattern.to_string());
    cmd_args.push(search_path.to_string_lossy().to_string());

    match tokio::process::Command::new("grep")
        .args(&cmd_args)
        .output()
        .await
    {
        Ok(output) => {
            let mut result = String::from_utf8_lossy(&output.stdout).to_string();
            if result.len() > 50_000 {
                result.truncate(50_000);
                result.push_str("\n... [truncated]");
            }
            if result.is_empty() {
                "No matches found.".into()
            } else {
                result
            }
        }
        Err(e) => format!("Error running grep: {e}"),
    }
}

async fn exec_bash(args: &serde_json::Value, workdir: &Path) -> String {
    let command = match args.get("command").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return "Error: missing 'command' parameter".into(),
    };

    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(workdir)
        .output()
        .await
    {
        Ok(output) => {
            let mut result = String::new();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("[stderr] ");
                result.push_str(&stderr);
            }

            let code = output.status.code().unwrap_or(-1);
            if code != 0 {
                result.push_str(&format!("\n[exit code: {code}]"));
            }

            if result.len() > 50_000 {
                result.truncate(50_000);
                result.push_str("\n... [truncated]");
            }

            if result.is_empty() {
                "(no output)".into()
            } else {
                result
            }
        }
        Err(e) => format!("Error running command: {e}"),
    }
}
