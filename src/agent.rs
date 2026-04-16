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
            let assistant_msg = self
                .client
                .chat_stream(&self.messages, &self.tools)
                .await?;

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
}
