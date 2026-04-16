use crate::client::LlmClient;
use crate::tools;
use crate::types::*;
use std::path::PathBuf;

const SYSTEM_PROMPT: &str = r#"You are a skilled software engineer. You have access to tools that let you read files, list directories, edit files, search code, and run shell commands.

When the user asks you to complete a task:
1. Explore the relevant code and understand the context first.
2. Plan your changes before making them.
3. Make precise, minimal edits — don't rewrite entire files when a targeted edit suffices.
4. Verify your work by reading files back or running tests.

Guidelines:
- Use read_file to understand code before editing.
- Use list_dir to explore project structure.
- Use grep to find relevant code across the project.
- Use edit_file with exact string matches to make targeted changes.
- Use bash for build, test, git, and other shell operations.
- Be concise in your responses."#;

pub struct Agent {
    client: LlmClient,
    messages: Vec<Message>,
    tools: Vec<ToolDef>,
    workdir: PathBuf,
    max_turns: usize,
}

impl Agent {
    pub fn new(client: LlmClient, workdir: PathBuf) -> Self {
        let tools = tools::tool_definitions();
        let messages = vec![Message::system(SYSTEM_PROMPT)];

        Self {
            client,
            messages,
            tools,
            workdir,
            max_turns: 50,
        }
    }

    /// Run one user turn through the agent loop.
    /// Keeps calling the model until it responds without tool calls.
    pub async fn run_turn(&mut self, user_input: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.messages.push(Message::user(user_input));

        for _ in 0..self.max_turns {
            let assistant_msg = self.client.chat_stream(&self.messages, &self.tools).await?;

            let has_tool_calls = assistant_msg.tool_calls.is_some();
            self.messages.push(assistant_msg.clone());

            if !has_tool_calls {
                // Model responded without tool calls — turn is complete
                break;
            }

            // Execute all tool calls (in parallel via tokio::join_all)
            let tool_calls = assistant_msg.tool_calls.as_ref().unwrap();
            let futures: Vec<_> = tool_calls
                .iter()
                .map(|tc| {
                    let tc = tc.clone();
                    let workdir = self.workdir.clone();
                    tokio::spawn(async move {
                        let result = tools::execute(&tc, &workdir).await;
                        (tc.id.clone(), tc.function.name.clone(), result)
                    })
                })
                .collect();

            for handle in futures {
                let (id, name, result) = handle.await?;
                eprintln!("\x1b[90m[tool: {name}] {} chars\x1b[0m", result.len());
                self.messages.push(Message::tool_result(&id, &result));
            }
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
        Agent::new(client, PathBuf::from("/tmp"))
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
    fn agent_new_has_five_tools() {
        let agent = make_agent();
        assert_eq!(agent.tools.len(), 5);
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
}
