use crate::client::LlmClient;
use crate::plan::{self, PlanBoard};
use crate::spinner::{self, Spinner, Style, ToolProgress};
use crate::tools;
use crate::types::*;
use std::path::{Path, PathBuf};

const SYSTEM_PROMPT: &str = r#"You are a skilled software engineer with direct access to tools. You MUST use your tools to accomplish tasks — never just describe what you would do.

PLANNING IS COMPULSORY:
Before touching any file or running any command, you MUST first respond with a numbered plan.
The plan must list every step you intend to take and why. Only after the plan is laid out should
you begin executing it with tool calls. Never skip the planning step — even for small tasks.

For code changes, use red-green-refactor when it applies:
  1. RED — Write a failing test that captures the desired behavior.
  2. GREEN — Write the minimum code to make the test pass.
  3. REFACTOR — Clean up while keeping tests green.
Include these phases in your plan when the task involves new features, bug fixes, or behavioral
changes. Skip red-green-refactor only for non-code tasks (file moves, config edits, exploration).

CRITICAL RULES:
- ALWAYS output a plan first, then call tools to execute it.
- Do not ask for permission or confirmation — plan, then do the work.
- You can call multiple tools in a single response.

Workflow:
1. Plan your approach (mandatory first response — text only, no tool calls).
2. Use list_dir and read_file to understand the codebase.
3. Use grep to search across files.
4. Write failing tests first when making code changes.
5. Use edit_file to create new files (set old_string to empty) or make targeted edits.
6. Use bash to run builds, tests, git commands, install dependencies, etc.
7. Verify by running tests and reading files back.

Tool tips:
- edit_file with empty old_string creates a new file (parent dirs are created automatically).
- edit_file with a non-empty old_string replaces that exact match. Provide enough context for a unique match.
- bash runs in the working directory. Use it for anything the other tools don't cover.
- Be concise in text responses. Let your tool calls do the talking."#;

/// Try to read STRAP.md from the working directory or its ancestors.
fn find_strap_md(workdir: &Path) -> Option<String> {
    let mut dir = workdir.to_path_buf();
    loop {
        let candidate = dir.join("STRAP.md");
        if let Ok(content) = std::fs::read_to_string(&candidate) {
            return Some(content);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Build the full system prompt, appending STRAP.md if found.
fn build_system_prompt(workdir: &Path) -> String {
    match find_strap_md(workdir) {
        Some(rules) => {
            format!("{SYSTEM_PROMPT}\n\n---\nProject rules (from STRAP.md):\n\n{rules}")
        }
        None => SYSTEM_PROMPT.to_string(),
    }
}

pub struct Agent {
    client: LlmClient,
    messages: Vec<Message>,
    tools: Vec<ToolDef>,
    workdir: PathBuf,
    max_turns: usize,
    board: PlanBoard,
}

impl Agent {
    pub fn new(client: LlmClient, workdir: PathBuf, board: PlanBoard) -> Self {
        let tools = tools::tool_definitions();
        let prompt = build_system_prompt(&workdir);

        if prompt.len() > SYSTEM_PROMPT.len() {
            eprintln!("  \x1b[32m\x1b[1m✓\x1b[0m \x1b[2mloaded\x1b[0m \x1b[36mSTRAP.md\x1b[0m");
        }

        let messages = vec![Message::system(&prompt)];

        Self {
            client,
            messages,
            tools,
            workdir,
            max_turns: 50,
            board,
        }
    }

    /// Run one user turn through the agent loop.
    /// Keeps calling the model until it responds without tool calls.
    pub async fn run_turn(&mut self, user_input: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.messages.push(Message::user(user_input));
        self.board.reset().await;

        let mut plan_parsed = false;
        let mut total_prompt = 0u64;
        let mut total_completion = 0u64;

        for _ in 0..self.max_turns {
            let (assistant_msg, usage) =
                self.client.chat_stream(&self.messages, &self.tools).await?;

            if let Some(u) = usage {
                total_prompt += u.prompt_tokens;
                total_completion += u.completion_tokens;
                self.board
                    .update_usage(u.prompt_tokens, u.completion_tokens)
                    .await;
            }

            if !plan_parsed {
                if let Some(ref content) = assistant_msg.content {
                    if let Some(steps) = plan::parse_plan(content) {
                        self.board.set_plan(steps).await;
                        plan_parsed = true;
                    }
                }
            }

            let has_tool_calls = assistant_msg.tool_calls.is_some();
            self.messages.push(assistant_msg.clone());

            if !has_tool_calls {
                break;
            }

            self.board.advance().await;

            let tool_calls = assistant_msg.tool_calls.as_ref().unwrap();
            let count = tool_calls.len();

            if count == 1 {
                let tc = &tool_calls[0];
                let commentary = tools::describe_call(tc);
                self.board
                    .update_activity(&format!("{} {}", tc.function.name, commentary))
                    .await;
                let spinner = Spinner::start_tool(&tc.function.name, &commentary, Style::Bounce);
                let result = tools::execute(tc, &self.workdir).await;
                spinner.stop().await;
                spinner::print_tool_done(
                    &tc.function.name,
                    &commentary,
                    &format!("{} chars", result.len()),
                );
                self.messages.push(Message::tool_result(&tc.id, &result));
            } else {
                let mut progress = ToolProgress::new(count);
                let board = self.board.clone();
                let futures: Vec<_> = tool_calls
                    .iter()
                    .map(|tc| {
                        let tc = tc.clone();
                        let commentary = tools::describe_call(&tc);
                        let workdir = self.workdir.clone();
                        let b = board.clone();
                        tokio::spawn(async move {
                            b.update_activity(&format!("{} {}", tc.function.name, commentary))
                                .await;
                            let result = tools::execute(&tc, &workdir).await;
                            (tc.id.clone(), tc.function.name.clone(), commentary, result)
                        })
                    })
                    .collect();

                for handle in futures {
                    let (id, name, commentary, result) = handle.await?;
                    progress.tick(&name, &commentary);
                    self.messages.push(Message::tool_result(&id, &result));
                }
                progress.finish();
                spinner::print_tools_done(count);
            }
        }

        self.board.complete_current().await;

        if let Err(e) = plan::write_history(&self.board, &self.workdir, user_input).await {
            eprintln!("\x1b[2m  warn: could not update TASKS.md: {e}\x1b[0m");
        }

        let total = total_prompt + total_completion;
        if total > 0 {
            spinner::print_usage(total_prompt, total_completion, total);
        }

        Ok(())
    }

    /// Compact old messages to stay within context limits.
    /// Keeps system prompt + last N messages.
    pub fn compact(&mut self, keep_last: usize) {
        if self.messages.len() <= keep_last + 1 {
            return;
        }
        let system = self.messages[0].clone();
        let tail = self.messages.split_off(self.messages.len() - keep_last);
        self.messages.clear();
        self.messages.push(system);
        self.messages.push(Message::user(
            "[Earlier conversation was compacted to save context. Continue from the most recent messages.]",
        ));
        self.messages.extend(tail);
    }

    /// Get a reference to the message history (for testing).
    #[cfg(test)]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Get the number of messages (for testing).
    #[cfg(test)]
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_agent() -> Agent {
        let client = LlmClient::new("http://localhost:0/v1", "test-key", "test-model");
        let board = PlanBoard::new();
        Agent::new(client, PathBuf::from("/tmp"), board)
    }

    // ── Constructor tests ──

    #[test]
    fn agent_new_starts_with_system_prompt() {
        let agent = make_agent();
        assert_eq!(agent.messages().len(), 1);
        assert_eq!(agent.messages()[0].role, "system");
        assert!(agent.messages()[0]
            .content
            .as_ref()
            .unwrap()
            .contains("software engineer"));
    }

    #[test]
    fn agent_new_has_six_tools() {
        let agent = make_agent();
        assert_eq!(agent.tools.len(), 6);
    }

    #[test]
    fn agent_new_has_max_turns_50() {
        let agent = make_agent();
        assert_eq!(agent.max_turns, 50);
    }

    // ── Compact tests ──

    #[test]
    fn compact_noop_when_few_messages() {
        let mut agent = make_agent();
        // Only system prompt — nothing to compact
        agent.compact(10);
        assert_eq!(agent.message_count(), 1);
        assert_eq!(agent.messages()[0].role, "system");
    }

    #[test]
    fn compact_noop_at_boundary() {
        let mut agent = make_agent();
        // Add exactly keep_last messages on top of system
        for i in 0..5 {
            agent.messages.push(Message::user(&format!("msg {i}")));
        }
        // Total: 6 messages (1 system + 5 user), keep_last = 5
        // 6 <= 5 + 1, so no compaction
        agent.compact(5);
        assert_eq!(agent.message_count(), 6);
    }

    #[test]
    fn compact_keeps_system_and_last_n() {
        let mut agent = make_agent();
        for i in 0..20 {
            agent.messages.push(Message::user(&format!("msg {i}")));
        }
        // Total: 21 messages
        agent.compact(5);
        // Should be: system + compaction notice + last 5 = 7
        assert_eq!(agent.message_count(), 7);
        assert_eq!(agent.messages()[0].role, "system");
        assert_eq!(agent.messages()[1].role, "user");
        assert!(agent.messages()[1]
            .content
            .as_ref()
            .unwrap()
            .contains("compacted"));
    }

    #[test]
    fn compact_preserves_system_prompt_content() {
        let mut agent = make_agent();
        let original_system = agent.messages()[0].content.clone();
        for i in 0..20 {
            agent.messages.push(Message::user(&format!("msg {i}")));
        }
        agent.compact(3);
        assert_eq!(agent.messages()[0].content, original_system);
    }

    #[test]
    fn compact_preserves_most_recent_messages() {
        let mut agent = make_agent();
        for i in 0..10 {
            agent.messages.push(Message::user(&format!("msg {i}")));
        }
        agent.compact(3);
        // Last 3 messages should be msg 7, 8, 9
        let msgs = agent.messages();
        let last3: Vec<&str> = msgs[msgs.len() - 3..]
            .iter()
            .map(|m| m.content.as_ref().unwrap().as_str())
            .collect();
        assert_eq!(last3, vec!["msg 7", "msg 8", "msg 9"]);
    }

    #[test]
    fn compact_with_keep_last_zero() {
        let mut agent = make_agent();
        for i in 0..5 {
            agent.messages.push(Message::user(&format!("msg {i}")));
        }
        // keep_last = 0, should keep just system + compaction notice
        agent.compact(0);
        assert_eq!(agent.message_count(), 2);
        assert_eq!(agent.messages()[0].role, "system");
        assert!(agent.messages()[1]
            .content
            .as_ref()
            .unwrap()
            .contains("compacted"));
    }

    #[test]
    fn compact_with_mixed_message_types() {
        let mut agent = make_agent();
        agent.messages.push(Message::user("question"));
        agent
            .messages
            .push(Message::assistant(Some("answer".into()), None));
        agent.messages.push(Message::user("follow up"));
        agent.messages.push(Message::assistant(
            None,
            Some(vec![ToolCall {
                id: "tc_1".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: "bash".into(),
                    arguments: "{}".into(),
                },
            }]),
        ));
        agent.messages.push(Message::tool_result("tc_1", "output"));
        agent
            .messages
            .push(Message::assistant(Some("done".into()), None));

        // Total: 7 messages, compact keeping last 2
        agent.compact(2);
        assert_eq!(agent.message_count(), 4); // system + notice + last 2
        let msgs = agent.messages();
        // Last message should be the "done" assistant message
        assert_eq!(msgs[3].content.as_ref().unwrap(), "done");
    }

    #[test]
    fn compact_twice_is_idempotent_when_small() {
        let mut agent = make_agent();
        for i in 0..3 {
            agent.messages.push(Message::user(&format!("msg {i}")));
        }
        agent.compact(5);
        let count_after_first = agent.message_count();
        agent.compact(5);
        assert_eq!(agent.message_count(), count_after_first);
    }

    // ── STRAP.md tests ──

    #[test]
    fn find_strap_md_returns_none_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_strap_md(dir.path()).is_none());
    }

    #[test]
    fn find_strap_md_finds_in_current_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("STRAP.md"), "# Rules\nBe fast.").unwrap();
        let content = find_strap_md(dir.path()).unwrap();
        assert!(content.contains("Be fast."));
    }

    #[test]
    fn find_strap_md_walks_up_to_parent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("STRAP.md"), "# Parent rules").unwrap();
        let child = dir.path().join("src");
        std::fs::create_dir(&child).unwrap();
        let content = find_strap_md(&child).unwrap();
        assert!(content.contains("Parent rules"));
    }

    #[test]
    fn build_system_prompt_without_strap_md() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = build_system_prompt(dir.path());
        assert!(prompt.contains("software engineer"));
        assert!(!prompt.contains("STRAP.md"));
    }

    #[test]
    fn build_system_prompt_with_strap_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("STRAP.md"), "# Custom rules\nAlways test.").unwrap();
        let prompt = build_system_prompt(dir.path());
        assert!(prompt.contains("software engineer"));
        assert!(prompt.contains("Always test."));
        assert!(prompt.contains("STRAP.md"));
    }

    #[test]
    fn agent_with_strap_md_includes_rules_in_prompt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("STRAP.md"), "# My rules\nNo dead code.").unwrap();
        let client = LlmClient::new("http://localhost:0/v1", "test-key", "test-model");
        let agent = Agent::new(client, dir.path().to_path_buf(), PlanBoard::new());
        let system_content = agent.messages()[0].content.as_ref().unwrap();
        assert!(system_content.contains("No dead code."));
    }
}
