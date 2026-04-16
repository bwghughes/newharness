use crate::types::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
                description: "Search for a regex pattern in files recursively using ripgrep. Returns matching lines with file paths and line numbers. Respects .gitignore.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Regex pattern to search for (case-insensitive). Use \\b for word boundaries."
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
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "web_search".into(),
                description: "Search the web via Tavily. Returns an AI-generated summary plus ranked results (title, URL, snippet). Use for library docs, recent APIs, error messages, or any external/up-to-date info — not codebase search.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of results (default: 5, max: 10)"
                        }
                    },
                    "required": ["query"]
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
        "web_search" => exec_web_search(&args).await,
        other => format!("Unknown tool: {other}"),
    }
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client")
    })
}

async fn exec_web_search(args: &serde_json::Value) -> String {
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.trim().is_empty() => q,
        _ => return "Error: missing 'query' parameter".into(),
    };

    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .clamp(1, 10);

    let api_key = match std::env::var("STRAPIN_SEARCH_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            return "Error: STRAPIN_SEARCH_API_KEY not set. Get a free key at https://tavily.com"
                .into()
        }
    };

    let body = json!({
        "api_key": api_key,
        "query": query,
        "search_depth": "basic",
        "max_results": max_results,
        "include_answer": true,
    });

    let response = match http_client()
        .post("https://api.tavily.com/search")
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("Error calling Tavily: {e}"),
    };

    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return format!("Tavily API error ({status}): {text}");
    }

    let data: serde_json::Value = match response.json().await {
        Ok(d) => d,
        Err(e) => return format!("Error parsing Tavily response: {e}"),
    };

    format_tavily_results(&data)
}

fn format_tavily_results(data: &serde_json::Value) -> String {
    let mut out = String::new();

    if let Some(answer) = data.get("answer").and_then(|v| v.as_str()) {
        if !answer.trim().is_empty() {
            out.push_str("Summary: ");
            out.push_str(answer);
            out.push_str("\n\n");
        }
    }

    if let Some(results) = data.get("results").and_then(|v| v.as_array()) {
        for (i, result) in results.iter().enumerate() {
            let title = result
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("(no title)");
            let url = result.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let content = result.get("content").and_then(|v| v.as_str()).unwrap_or("");
            out.push_str(&format!(
                "[{}] {title}\n    {url}\n    {content}\n\n",
                i + 1
            ));
        }
    }

    if out.trim().is_empty() {
        return "No results.".into();
    }

    if out.len() > 50_000 {
        out.truncate(50_000);
        out.push_str("\n... [truncated]");
    }

    out
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
        Ok(()) => format!(
            "Edited {} ({} bytes written)",
            path.display(),
            new_content.len()
        ),
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
        "-n".to_string(),
        "-i".to_string(),
        "--no-heading".to_string(),
    ];

    if let Some(glob) = args.get("glob").and_then(|v| v.as_str()) {
        cmd_args.push("-g".to_string());
        cmd_args.push(glob.to_string());
    }

    cmd_args.push("--".to_string());
    cmd_args.push(pattern.to_string());
    cmd_args.push(search_path.to_string_lossy().to_string());

    match tokio::process::Command::new("rg")
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
        Err(e) => format!("Error running rg: {e}"),
    }
}

/// Resolve a user-supplied path relative to the working directory.
/// Exposed for testing.
#[cfg(test)]
pub(crate) fn resolve_path_pub(raw: &str, workdir: &Path) -> PathBuf {
    resolve_path(raw, workdir)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    // ── tool_definitions tests ──

    #[test]
    fn tool_definitions_returns_six_tools() {
        let defs = tool_definitions();
        assert_eq!(defs.len(), 6);
    }

    #[test]
    fn tool_definitions_names_are_correct() {
        let defs = tool_definitions();
        let names: Vec<&str> = defs.iter().map(|t| t.function.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "list_dir",
                "edit_file",
                "grep",
                "bash",
                "web_search"
            ]
        );
    }

    #[test]
    fn tool_definitions_all_have_function_type() {
        for def in tool_definitions() {
            assert_eq!(def.kind, "function");
        }
    }

    #[test]
    fn tool_definitions_all_have_descriptions() {
        for def in tool_definitions() {
            assert!(!def.function.description.is_empty());
        }
    }

    #[test]
    fn tool_definitions_parameters_are_objects() {
        for def in tool_definitions() {
            assert_eq!(def.function.parameters["type"], "object");
            assert!(def.function.parameters.get("properties").is_some());
        }
    }

    #[test]
    fn tool_definitions_serialize_to_valid_json() {
        let defs = tool_definitions();
        let json = serde_json::to_string(&defs).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 6);
    }

    // ── resolve_path tests ──

    #[test]
    fn resolve_path_relative() {
        let wd = Path::new("/home/user/project");
        let p = resolve_path_pub("src/main.rs", wd);
        assert_eq!(p, PathBuf::from("/home/user/project/src/main.rs"));
    }

    #[test]
    fn resolve_path_absolute() {
        let wd = Path::new("/home/user/project");
        let p = resolve_path_pub("/etc/passwd", wd);
        assert_eq!(p, PathBuf::from("/etc/passwd"));
    }

    // ── read_file tests ──

    #[tokio::test]
    async fn read_file_returns_numbered_lines() {
        let dir = tmp();
        fs::write(
            dir.path().join("hello.txt"),
            "line one\nline two\nline three",
        )
        .unwrap();

        let args = json!({"path": "hello.txt"});
        let result = exec_read_file(&args, dir.path()).await;

        assert!(result.contains("   1\tline one"));
        assert!(result.contains("   2\tline two"));
        assert!(result.contains("   3\tline three"));
    }

    #[tokio::test]
    async fn read_file_error_on_missing_file() {
        let dir = tmp();
        let args = json!({"path": "nonexistent.txt"});
        let result = exec_read_file(&args, dir.path()).await;
        assert!(result.starts_with("Error reading"));
    }

    #[tokio::test]
    async fn read_file_missing_path_param() {
        let dir = tmp();
        let args = json!({});
        let result = exec_read_file(&args, dir.path()).await;
        assert!(result.contains("missing 'path'"));
    }

    #[tokio::test]
    async fn read_file_handles_empty_file() {
        let dir = tmp();
        fs::write(dir.path().join("empty.txt"), "").unwrap();
        let args = json!({"path": "empty.txt"});
        let result = exec_read_file(&args, dir.path()).await;
        // Empty file — no lines
        assert!(result.is_empty() || result.trim().is_empty());
    }

    #[tokio::test]
    async fn read_file_handles_unicode() {
        let dir = tmp();
        fs::write(dir.path().join("unicode.txt"), "日本語テスト\némojis 🎉").unwrap();
        let args = json!({"path": "unicode.txt"});
        let result = exec_read_file(&args, dir.path()).await;
        assert!(result.contains("日本語テスト"));
        assert!(result.contains("🎉"));
    }

    #[tokio::test]
    async fn read_file_truncates_large_content() {
        let dir = tmp();
        // Create a file >100KB
        let big = "x".repeat(200_000);
        fs::write(dir.path().join("big.txt"), &big).unwrap();
        let args = json!({"path": "big.txt"});
        let result = exec_read_file(&args, dir.path()).await;
        assert!(result.contains("[truncated at 100KB]"));
        assert!(result.len() < 110_000);
    }

    // ── list_dir tests ──

    #[tokio::test]
    async fn list_dir_shows_files_and_dirs() {
        let dir = tmp();
        fs::write(dir.path().join("file.txt"), "").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let args = json!({});
        let result = exec_list_dir(&args, dir.path()).await;

        assert!(result.contains("file.txt"));
        assert!(result.contains("subdir/"));
    }

    #[tokio::test]
    async fn list_dir_skips_hidden_files() {
        let dir = tmp();
        fs::write(dir.path().join(".hidden"), "").unwrap();
        fs::write(dir.path().join("visible.txt"), "").unwrap();

        let args = json!({});
        let result = exec_list_dir(&args, dir.path()).await;

        assert!(!result.contains(".hidden"));
        assert!(result.contains("visible.txt"));
    }

    #[tokio::test]
    async fn list_dir_sorted_output() {
        let dir = tmp();
        fs::write(dir.path().join("zebra.txt"), "").unwrap();
        fs::write(dir.path().join("alpha.txt"), "").unwrap();
        fs::write(dir.path().join("middle.txt"), "").unwrap();

        let args = json!({});
        let result = exec_list_dir(&args, dir.path()).await;
        let lines: Vec<&str> = result.lines().collect();

        assert_eq!(lines, vec!["alpha.txt", "middle.txt", "zebra.txt"]);
    }

    #[tokio::test]
    async fn list_dir_with_explicit_path() {
        let dir = tmp();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("inner.txt"), "").unwrap();

        let args = json!({"path": "sub"});
        let result = exec_list_dir(&args, dir.path()).await;
        assert!(result.contains("inner.txt"));
    }

    #[tokio::test]
    async fn list_dir_error_on_missing_dir() {
        let dir = tmp();
        let args = json!({"path": "no_such_dir"});
        let result = exec_list_dir(&args, dir.path()).await;
        assert!(result.starts_with("Error listing"));
    }

    #[tokio::test]
    async fn list_dir_empty_directory() {
        let dir = tmp();
        let args = json!({});
        let result = exec_list_dir(&args, dir.path()).await;
        assert!(result.is_empty());
    }

    // ── edit_file tests ──

    #[tokio::test]
    async fn edit_file_creates_new_file() {
        let dir = tmp();
        let args = json!({
            "path": "new.txt",
            "old_string": "",
            "new_string": "hello world"
        });
        let result = exec_edit_file(&args, dir.path()).await;
        assert!(result.contains("Created"));

        let content = fs::read_to_string(dir.path().join("new.txt")).unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn edit_file_creates_parent_directories() {
        let dir = tmp();
        let args = json!({
            "path": "deep/nested/dir/file.txt",
            "old_string": "",
            "new_string": "content"
        });
        let result = exec_edit_file(&args, dir.path()).await;
        assert!(result.contains("Created"));
        assert!(dir.path().join("deep/nested/dir/file.txt").exists());
    }

    #[tokio::test]
    async fn edit_file_replaces_exact_match() {
        let dir = tmp();
        fs::write(dir.path().join("test.txt"), "foo bar baz").unwrap();

        let args = json!({
            "path": "test.txt",
            "old_string": "bar",
            "new_string": "qux"
        });
        let result = exec_edit_file(&args, dir.path()).await;
        assert!(result.contains("Edited"));

        let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "foo qux baz");
    }

    #[tokio::test]
    async fn edit_file_replaces_multiline_match() {
        let dir = tmp();
        fs::write(dir.path().join("test.txt"), "line1\nline2\nline3").unwrap();

        let args = json!({
            "path": "test.txt",
            "old_string": "line1\nline2",
            "new_string": "replaced"
        });
        let result = exec_edit_file(&args, dir.path()).await;
        assert!(result.contains("Edited"));

        let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
        assert_eq!(content, "replaced\nline3");
    }

    #[tokio::test]
    async fn edit_file_errors_when_not_found() {
        let dir = tmp();
        fs::write(dir.path().join("test.txt"), "hello world").unwrap();

        let args = json!({
            "path": "test.txt",
            "old_string": "nonexistent string",
            "new_string": "replacement"
        });
        let result = exec_edit_file(&args, dir.path()).await;
        assert!(result.contains("old_string not found"));
    }

    #[tokio::test]
    async fn edit_file_errors_on_multiple_matches() {
        let dir = tmp();
        fs::write(dir.path().join("test.txt"), "aaa bbb aaa").unwrap();

        let args = json!({
            "path": "test.txt",
            "old_string": "aaa",
            "new_string": "ccc"
        });
        let result = exec_edit_file(&args, dir.path()).await;
        assert!(result.contains("found 2 times"));
    }

    #[tokio::test]
    async fn edit_file_missing_path() {
        let dir = tmp();
        let args = json!({"old_string": "a", "new_string": "b"});
        let result = exec_edit_file(&args, dir.path()).await;
        assert!(result.contains("missing 'path'"));
    }

    #[tokio::test]
    async fn edit_file_error_reading_nonexistent() {
        let dir = tmp();
        let args = json!({
            "path": "nope.txt",
            "old_string": "x",
            "new_string": "y"
        });
        let result = exec_edit_file(&args, dir.path()).await;
        assert!(result.contains("Error reading"));
    }

    #[tokio::test]
    async fn edit_file_reports_bytes_written() {
        let dir = tmp();
        fs::write(dir.path().join("test.txt"), "old content").unwrap();
        let args = json!({
            "path": "test.txt",
            "old_string": "old content",
            "new_string": "new content here"
        });
        let result = exec_edit_file(&args, dir.path()).await;
        assert!(result.contains("bytes written"));
    }

    // ── grep tests ──

    #[tokio::test]
    async fn grep_finds_matching_lines() {
        let dir = tmp();
        fs::write(dir.path().join("test.rs"), "fn main() {}\nfn helper() {}").unwrap();

        let args = json!({"pattern": "fn main"});
        let result = exec_grep(&args, dir.path()).await;
        assert!(result.contains("fn main"));
    }

    #[tokio::test]
    async fn grep_case_insensitive() {
        let dir = tmp();
        fs::write(dir.path().join("test.txt"), "Hello World\nhello world").unwrap();

        let args = json!({"pattern": "HELLO"});
        let result = exec_grep(&args, dir.path()).await;
        // Should find both lines due to -i flag
        assert!(result.contains("Hello World"));
        assert!(result.contains("hello world"));
    }

    #[tokio::test]
    async fn grep_with_glob_filter() {
        let dir = tmp();
        fs::write(dir.path().join("test.rs"), "fn target() {}").unwrap();
        fs::write(dir.path().join("test.txt"), "fn target() {}").unwrap();

        let args = json!({"pattern": "target", "glob": "*.rs"});
        let result = exec_grep(&args, dir.path()).await;
        assert!(result.contains("test.rs"));
        assert!(!result.contains("test.txt"));
    }

    #[tokio::test]
    async fn grep_no_matches() {
        let dir = tmp();
        fs::write(dir.path().join("test.txt"), "nothing relevant here").unwrap();

        let args = json!({"pattern": "zzzznonexistent"});
        let result = exec_grep(&args, dir.path()).await;
        assert_eq!(result, "No matches found.");
    }

    #[tokio::test]
    async fn grep_missing_pattern() {
        let dir = tmp();
        let args = json!({});
        let result = exec_grep(&args, dir.path()).await;
        assert!(result.contains("missing 'pattern'"));
    }

    #[tokio::test]
    async fn grep_in_subdirectory() {
        let dir = tmp();
        let sub = dir.path().join("src");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("lib.rs"), "pub fn search_target() {}").unwrap();

        let args = json!({"pattern": "search_target", "path": "src"});
        let result = exec_grep(&args, dir.path()).await;
        assert!(result.contains("search_target"));
    }

    // ── bash tests ──

    #[tokio::test]
    async fn bash_runs_simple_command() {
        let dir = tmp();
        let args = json!({"command": "echo hello"});
        let result = exec_bash(&args, dir.path()).await;
        assert_eq!(result.trim(), "hello");
    }

    #[tokio::test]
    async fn bash_captures_stdout_and_stderr() {
        let dir = tmp();
        let args = json!({"command": "echo out && echo err >&2"});
        let result = exec_bash(&args, dir.path()).await;
        assert!(result.contains("out"));
        assert!(result.contains("[stderr] err"));
    }

    #[tokio::test]
    async fn bash_reports_nonzero_exit_code() {
        let dir = tmp();
        let args = json!({"command": "exit 42"});
        let result = exec_bash(&args, dir.path()).await;
        assert!(result.contains("[exit code: 42]"));
    }

    #[tokio::test]
    async fn bash_uses_workdir() {
        let dir = tmp();
        let args = json!({"command": "pwd"});
        let result = exec_bash(&args, dir.path()).await;
        assert!(result
            .trim()
            .ends_with(dir.path().file_name().unwrap().to_str().unwrap()));
    }

    #[tokio::test]
    async fn bash_missing_command() {
        let dir = tmp();
        let args = json!({});
        let result = exec_bash(&args, dir.path()).await;
        assert!(result.contains("missing 'command'"));
    }

    #[tokio::test]
    async fn bash_no_output_returns_placeholder() {
        let dir = tmp();
        let args = json!({"command": "true"});
        let result = exec_bash(&args, dir.path()).await;
        assert_eq!(result, "(no output)");
    }

    #[tokio::test]
    async fn bash_piped_commands() {
        let dir = tmp();
        let args = json!({"command": "echo 'abc\ndef\nghi' | grep def"});
        let result = exec_bash(&args, dir.path()).await;
        assert_eq!(result.trim(), "def");
    }

    // ── web_search tests ──

    #[tokio::test]
    async fn web_search_missing_query() {
        let args = json!({});
        let result = exec_web_search(&args).await;
        assert!(result.contains("missing 'query'"));
    }

    #[tokio::test]
    async fn web_search_empty_query() {
        let args = json!({"query": "   "});
        let result = exec_web_search(&args).await;
        assert!(result.contains("missing 'query'"));
    }

    #[test]
    fn format_tavily_includes_summary() {
        let data = json!({
            "answer": "Rust is a systems programming language.",
            "results": []
        });
        let out = format_tavily_results(&data);
        assert!(out.contains("Summary: Rust is a systems"));
    }

    #[test]
    fn format_tavily_lists_results() {
        let data = json!({
            "answer": "",
            "results": [
                {"title": "Rust Book", "url": "https://doc.rust-lang.org/book/", "content": "The Rust Programming Language."},
                {"title": "Async Book", "url": "https://rust-lang.github.io/async-book/", "content": "Async Rust guide."}
            ]
        });
        let out = format_tavily_results(&data);
        assert!(out.contains("[1] Rust Book"));
        assert!(out.contains("https://doc.rust-lang.org/book/"));
        assert!(out.contains("[2] Async Book"));
        assert!(out.contains("Async Rust guide."));
    }

    #[test]
    fn format_tavily_empty_returns_no_results() {
        let data = json!({"answer": "", "results": []});
        let out = format_tavily_results(&data);
        assert_eq!(out, "No results.");
    }

    #[test]
    fn format_tavily_handles_missing_fields() {
        let data = json!({
            "results": [
                {"url": "https://example.com"}
            ]
        });
        let out = format_tavily_results(&data);
        assert!(out.contains("(no title)"));
        assert!(out.contains("https://example.com"));
    }

    #[test]
    fn format_tavily_truncates_large_output() {
        let big_content = "x".repeat(60_000);
        let data = json!({
            "answer": "",
            "results": [
                {"title": "t", "url": "u", "content": big_content}
            ]
        });
        let out = format_tavily_results(&data);
        assert!(out.len() <= 50_100);
        assert!(out.contains("[truncated]"));
    }

    // ── execute dispatch tests ──

    #[tokio::test]
    async fn execute_dispatches_read_file() {
        let dir = tmp();
        fs::write(dir.path().join("dispatch.txt"), "contents").unwrap();

        let tc = ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: r#"{"path":"dispatch.txt"}"#.into(),
            },
        };
        let result = execute(&tc, dir.path()).await;
        assert!(result.contains("contents"));
    }

    #[tokio::test]
    async fn execute_dispatches_list_dir() {
        let dir = tmp();
        fs::write(dir.path().join("a.txt"), "").unwrap();

        let tc = ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "list_dir".into(),
                arguments: r#"{}"#.into(),
            },
        };
        let result = execute(&tc, dir.path()).await;
        assert!(result.contains("a.txt"));
    }

    #[tokio::test]
    async fn execute_dispatches_edit_file() {
        let dir = tmp();
        let tc = ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "edit_file".into(),
                arguments: r#"{"path":"new.txt","old_string":"","new_string":"created"}"#.into(),
            },
        };
        let result = execute(&tc, dir.path()).await;
        assert!(result.contains("Created"));
    }

    #[tokio::test]
    async fn execute_dispatches_bash() {
        let dir = tmp();
        let tc = ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: r#"{"command":"echo dispatch_test"}"#.into(),
            },
        };
        let result = execute(&tc, dir.path()).await;
        assert!(result.contains("dispatch_test"));
    }

    #[tokio::test]
    async fn execute_unknown_tool() {
        let dir = tmp();
        let tc = ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "unknown_tool".into(),
                arguments: "{}".into(),
            },
        };
        let result = execute(&tc, dir.path()).await;
        assert!(result.contains("Unknown tool: unknown_tool"));
    }

    #[tokio::test]
    async fn execute_handles_invalid_json_arguments() {
        let dir = tmp();
        let tc = ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: "not json".into(),
            },
        };
        let result = execute(&tc, dir.path()).await;
        assert!(result.contains("Error parsing arguments"));
    }
}
