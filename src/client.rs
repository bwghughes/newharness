use crate::types::*;
use futures_util::StreamExt;
use reqwest::Client;
use std::io::{self, Write};

/// Lightweight OpenAI-compatible streaming client.
/// Works with any endpoint that speaks the /v1/chat/completions SSE protocol
/// (OpenAI, Anthropic via proxy, Ollama, vLLM, LiteLLM, etc.).
pub struct LlmClient {
    http: Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl LlmClient {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let http = Client::builder()
            .pool_max_idle_per_host(4)
            .tcp_nodelay(true)
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    /// Stream a chat completion, printing tokens to stdout in real-time.
    /// Returns the fully assembled assistant message (content + tool_calls).
    pub async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
    ) -> Result<Message, Box<dyn std::error::Error>> {
        let url = format!("{}/chat/completions", self.base_url);

        let tools_param = if tools.is_empty() {
            None
        } else {
            Some(tools.to_vec())
        };

        let req = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools_param,
            stream: true,
            temperature: Some(0.0),
            max_tokens: Some(16384),
        };

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("API error {status}: {body}").into());
        }

        let mut content_buf = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut stdout = io::stdout().lock();

        let mut stream = resp.bytes_stream();

        // SSE line buffer — data can arrive in arbitrary chunk boundaries
        let mut line_buf = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let text = String::from_utf8_lossy(&chunk);

            line_buf.push_str(&text);

            // Process complete lines
            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].trim().to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                let data = if let Some(stripped) = line.strip_prefix("data: ") {
                    stripped.trim()
                } else {
                    continue;
                };

                if data == "[DONE]" {
                    break;
                }

                let chunk: StreamChunk = match serde_json::from_str(data) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                for choice in &chunk.choices {
                    // Stream content tokens to stdout
                    if let Some(ref c) = choice.delta.content {
                        content_buf.push_str(c);
                        let _ = write!(stdout, "{c}");
                        let _ = stdout.flush();
                    }

                    // Accumulate tool call deltas
                    if let Some(ref tcs) = choice.delta.tool_calls {
                        for tc in tcs {
                            let idx = tc.index;

                            // Extend vec if needed
                            while tool_calls.len() <= idx {
                                tool_calls.push(ToolCall {
                                    id: String::new(),
                                    kind: "function".into(),
                                    function: FunctionCall {
                                        name: String::new(),
                                        arguments: String::new(),
                                    },
                                });
                            }

                            if let Some(ref id) = tc.id {
                                tool_calls[idx].id = id.clone();
                            }
                            if let Some(ref f) = tc.function {
                                if let Some(ref name) = f.name {
                                    tool_calls[idx].function.name = name.clone();
                                }
                                if let Some(ref args) = f.arguments {
                                    tool_calls[idx].function.arguments.push_str(args);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Newline after streamed content
        if !content_buf.is_empty() {
            let _ = writeln!(stdout);
        }

        let content = if content_buf.is_empty() {
            None
        } else {
            Some(content_buf)
        };
        let tc = if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        };

        Ok(Message::assistant(content, tc))
    }
}
