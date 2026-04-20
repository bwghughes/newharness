use crate::spinner::{Spinner, Style};
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

/// Accumulates streamed SSE deltas into a complete assistant message.
/// Extracted from the streaming loop for testability.
pub(crate) struct StreamAssembler {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

/// Result of processing a single SSE data line.
#[derive(Debug, PartialEq)]
pub(crate) enum SseEvent {
    /// A content token to display
    ContentToken(String),
    /// A tool call delta was accumulated (no display)
    ToolCallDelta,
    /// Nothing actionable in this chunk
    Ignored,
    /// The stream signaled completion
    Done,
}

impl StreamAssembler {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            tool_calls: Vec::new(),
            usage: None,
        }
    }

    /// Process a raw SSE line (e.g. "data: {...}").
    /// Returns what happened so the caller can decide whether to print/continue.
    pub fn process_sse_line(&mut self, line: &str) -> SseEvent {
        let line = line.trim();

        if line.is_empty() || line.starts_with(':') {
            return SseEvent::Ignored;
        }

        let data = match line.strip_prefix("data: ") {
            Some(d) => d.trim(),
            None => return SseEvent::Ignored,
        };

        if data == "[DONE]" {
            return SseEvent::Done;
        }

        let chunk: StreamChunk = match serde_json::from_str(data) {
            Ok(c) => c,
            Err(e) => {
                if std::env::var("STRAPIN_VERBOSE").is_ok() {
                    eprintln!("\x1b[90m[debug] SSE parse error: {e} | data: {data}\x1b[0m");
                }
                return SseEvent::Ignored;
            }
        };

        if let Some(u) = chunk.usage {
            self.usage = Some(u);
        }

        let mut got_content = false;
        let mut got_tool = false;

        for choice in &chunk.choices {
            if let Some(ref c) = choice.delta.content {
                self.content.push_str(c);
                got_content = true;
            }

            if let Some(ref tcs) = choice.delta.tool_calls {
                for tc in tcs {
                    if std::env::var("STRAPIN_VERBOSE").is_ok() {
                        eprintln!(
                            "\x1b[90m[debug] tool_call delta: idx={} id={:?} name={:?} args={:?}\x1b[0m",
                            tc.index,
                            tc.id,
                            tc.function.as_ref().and_then(|f| f.name.as_ref()),
                            tc.function.as_ref().and_then(|f| f.arguments.as_ref()).map(|a| &a[..a.len().min(60)]),
                        );
                    }
                    self.apply_tool_call_delta(tc);
                }
                got_tool = true;
            }
        }

        if got_content {
            // Return the latest token (from the last choice that had content)
            let token = chunk
                .choices
                .iter()
                .filter_map(|c| c.delta.content.as_ref())
                .next_back()
                .unwrap()
                .clone();
            SseEvent::ContentToken(token)
        } else if got_tool {
            SseEvent::ToolCallDelta
        } else {
            SseEvent::Ignored
        }
    }

    /// Apply a single delta tool call to the accumulator.
    pub fn apply_tool_call_delta(&mut self, delta: &DeltaToolCall) {
        let idx = delta.index;

        // Extend vec if needed
        while self.tool_calls.len() <= idx {
            self.tool_calls.push(ToolCall {
                id: String::new(),
                kind: "function".into(),
                function: FunctionCall {
                    name: String::new(),
                    arguments: String::new(),
                },
            });
        }

        if let Some(ref id) = delta.id {
            self.tool_calls[idx].id = id.clone();
        }
        if let Some(ref f) = delta.function {
            if let Some(ref name) = f.name {
                if !name.is_empty() {
                    self.tool_calls[idx].function.name = name.clone();
                }
            }
            if let Some(ref args) = f.arguments {
                self.tool_calls[idx].function.arguments.push_str(args);
            }
        }
    }

    /// Consume the assembler and produce the final assistant message.
    /// Filters out incomplete tool calls (empty name = filler entry) and
    /// generates fallback IDs for providers that don't include them.
    pub fn finish(self) -> (Message, Option<Usage>) {
        let content = if self.content.is_empty() {
            None
        } else {
            Some(self.content)
        };

        let valid_calls: Vec<ToolCall> = self
            .tool_calls
            .into_iter()
            .enumerate()
            .filter(|(_, tc)| !tc.function.name.is_empty() || !tc.function.arguments.is_empty())
            .map(|(i, mut tc)| {
                if tc.id.is_empty() {
                    tc.id = format!("call_{i}");
                }
                tc
            })
            .collect();

        let tc = if valid_calls.is_empty() {
            None
        } else {
            Some(valid_calls)
        };
        (Message::assistant(content, tc), self.usage)
    }
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
    ) -> Result<(Message, Option<Usage>), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}/chat/completions", self.base_url);

        let (tools_param, tool_choice) = if tools.is_empty() {
            (None, None)
        } else {
            (Some(tools.to_vec()), Some(serde_json::json!("auto")))
        };

        let req = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools_param,
            tool_choice,
            stream: true,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
            temperature: Some(0.0),
            max_tokens: Some(16384),
        };

        if std::env::var("STRAPIN_VERBOSE").is_ok() {
            let tool_names: Vec<&str> = req
                .tools
                .as_ref()
                .map(|t| t.iter().map(|td| td.function.name.as_str()).collect())
                .unwrap_or_default();
            eprintln!(
                "\x1b[90m[debug] POST {} | model={} | tools=[{}] | tool_choice={} | messages={}\x1b[0m",
                url,
                req.model,
                tool_names.join(", "),
                req.tool_choice.as_ref().map(|v| v.to_string()).unwrap_or("none".into()),
                req.messages.len(),
            );
        }

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

        let mut assembler = StreamAssembler::new();
        let mut stream = resp.bytes_stream();
        let mut line_buf = String::new();

        let mut thinking_spinner = Some(Spinner::start("thinking", Style::Braille));
        let mut first_output = true;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let text = String::from_utf8_lossy(&chunk);
            line_buf.push_str(&text);

            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                match assembler.process_sse_line(&line) {
                    SseEvent::ContentToken(ref token) => {
                        if first_output {
                            if let Some(s) = thinking_spinner.take() {
                                s.stop().await;
                            }
                            first_output = false;
                        }
                        let mut out = io::stdout().lock();
                        let _ = write!(out, "{token}");
                        let _ = out.flush();
                    }
                    SseEvent::ToolCallDelta if first_output => {
                        if let Some(s) = thinking_spinner.take() {
                            s.stop().await;
                        }
                        first_output = false;
                    }
                    SseEvent::Done => break,
                    _ => {}
                }
            }
        }

        if let Some(s) = thinking_spinner.take() {
            s.stop().await;
        }

        if !assembler.content.is_empty() {
            let _ = writeln!(io::stdout().lock());
        }

        let verbose = std::env::var("STRAPIN_VERBOSE").is_ok();

        if verbose {
            for (i, tc) in assembler.tool_calls.iter().enumerate() {
                eprintln!(
                    "\x1b[90m[debug] raw_tool_call[{i}]: id=\"{}\" name=\"{}\" args_len={}\x1b[0m",
                    tc.id,
                    tc.function.name,
                    tc.function.arguments.len(),
                );
            }
        }

        let (msg, usage) = assembler.finish();

        if verbose {
            if let Some(ref tcs) = msg.tool_calls {
                for tc in tcs {
                    eprintln!(
                        "\x1b[90m[debug] tool_call id={} name={} args={}...\x1b[0m",
                        tc.id,
                        tc.function.name,
                        &tc.function.arguments[..tc.function.arguments.len().min(80)]
                    );
                }
            } else {
                eprintln!("\x1b[90m[debug] no tool_calls in response\x1b[0m");
            }
            if let Some(ref u) = usage {
                eprintln!(
                    "\x1b[90m[debug] usage: prompt={} completion={} total={}\x1b[0m",
                    u.prompt_tokens, u.completion_tokens, u.total_tokens
                );
            }
        }

        Ok((msg, usage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── StreamAssembler tests ──

    #[test]
    fn assembler_new_is_empty() {
        let a = StreamAssembler::new();
        assert!(a.content.is_empty());
        assert!(a.tool_calls.is_empty());
    }

    #[test]
    fn assembler_finish_empty_produces_none_fields() {
        let a = StreamAssembler::new();
        let (msg, _usage) = a.finish();
        assert_eq!(msg.role, "assistant");
        assert!(msg.content.is_none());
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn assembler_finish_with_content() {
        let mut a = StreamAssembler::new();
        a.content = "hello world".into();
        let (msg, _usage) = a.finish();
        assert_eq!(msg.content.unwrap(), "hello world");
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn assembler_finish_with_tool_calls() {
        let mut a = StreamAssembler::new();
        a.tool_calls.push(ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: r#"{"path":"test.rs"}"#.into(),
            },
        });
        let (msg, _usage) = a.finish();
        assert!(msg.content.is_none());
        let tcs = msg.tool_calls.unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "read_file");
    }

    // ── process_sse_line tests ──

    #[test]
    fn sse_ignores_empty_line() {
        let mut a = StreamAssembler::new();
        assert_eq!(a.process_sse_line(""), SseEvent::Ignored);
        assert_eq!(a.process_sse_line("   "), SseEvent::Ignored);
    }

    #[test]
    fn sse_ignores_comment_line() {
        let mut a = StreamAssembler::new();
        assert_eq!(a.process_sse_line(": keep-alive"), SseEvent::Ignored);
    }

    #[test]
    fn sse_ignores_non_data_line() {
        let mut a = StreamAssembler::new();
        assert_eq!(a.process_sse_line("event: message"), SseEvent::Ignored);
    }

    #[test]
    fn sse_detects_done() {
        let mut a = StreamAssembler::new();
        assert_eq!(a.process_sse_line("data: [DONE]"), SseEvent::Done);
    }

    #[test]
    fn sse_ignores_invalid_json() {
        let mut a = StreamAssembler::new();
        assert_eq!(
            a.process_sse_line("data: {not valid json}"),
            SseEvent::Ignored
        );
    }

    #[test]
    fn sse_processes_content_delta() {
        let mut a = StreamAssembler::new();
        let line = r#"data: {"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let result = a.process_sse_line(line);
        assert_eq!(result, SseEvent::ContentToken("Hello".into()));
        assert_eq!(a.content, "Hello");
    }

    #[test]
    fn sse_accumulates_multiple_content_deltas() {
        let mut a = StreamAssembler::new();
        let line1 = r#"data: {"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let line2 = r#"data: {"choices":[{"delta":{"content":" world"},"finish_reason":null}]}"#;
        a.process_sse_line(line1);
        a.process_sse_line(line2);
        assert_eq!(a.content, "Hello world");
    }

    #[test]
    fn sse_processes_tool_call_delta_with_id_and_name() {
        let mut a = StreamAssembler::new();
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_123","type":"function","function":{"name":"read_file","arguments":""}}]},"finish_reason":null}]}"#;
        let result = a.process_sse_line(line);
        assert_eq!(result, SseEvent::ToolCallDelta);
        assert_eq!(a.tool_calls.len(), 1);
        assert_eq!(a.tool_calls[0].id, "call_123");
        assert_eq!(a.tool_calls[0].function.name, "read_file");
    }

    #[test]
    fn sse_accumulates_tool_call_arguments_across_deltas() {
        let mut a = StreamAssembler::new();
        let line1 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"pa"}}]},"finish_reason":null}]}"#;
        let line2 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"test.rs\"}"}}]},"finish_reason":null}]}"#;
        a.process_sse_line(line1);
        a.process_sse_line(line2);
        assert_eq!(a.tool_calls.len(), 1);
        assert_eq!(a.tool_calls[0].function.arguments, r#"{"path":"test.rs"}"#);
    }

    #[test]
    fn sse_handles_multiple_concurrent_tool_calls() {
        let mut a = StreamAssembler::new();
        let line1 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":null}]}"#;
        let line2 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_2","type":"function","function":{"name":"list_dir","arguments":"{}"}}]},"finish_reason":null}]}"#;
        a.process_sse_line(line1);
        a.process_sse_line(line2);
        assert_eq!(a.tool_calls.len(), 2);
        assert_eq!(a.tool_calls[0].function.name, "read_file");
        assert_eq!(a.tool_calls[1].function.name, "list_dir");
    }

    #[test]
    fn sse_handles_empty_choices() {
        let mut a = StreamAssembler::new();
        let line = r#"data: {"choices":[]}"#;
        assert_eq!(a.process_sse_line(line), SseEvent::Ignored);
    }

    #[test]
    fn sse_handles_delta_with_role_only() {
        let mut a = StreamAssembler::new();
        let line = r#"data: {"choices":[{"delta":{"role":"assistant"},"finish_reason":null}]}"#;
        assert_eq!(a.process_sse_line(line), SseEvent::Ignored);
    }

    // ── apply_tool_call_delta tests ──

    #[test]
    fn delta_creates_new_entry_at_index() {
        let mut a = StreamAssembler::new();
        let delta = DeltaToolCall {
            index: 0,
            id: Some("tc_1".into()),
            kind: Some("function".into()),
            function: Some(DeltaFunction {
                name: Some("bash".into()),
                arguments: Some("{".into()),
            }),
        };
        a.apply_tool_call_delta(&delta);
        assert_eq!(a.tool_calls.len(), 1);
        assert_eq!(a.tool_calls[0].id, "tc_1");
        assert_eq!(a.tool_calls[0].function.name, "bash");
        assert_eq!(a.tool_calls[0].function.arguments, "{");
    }

    #[test]
    fn delta_extends_vec_for_sparse_index() {
        let mut a = StreamAssembler::new();
        let delta = DeltaToolCall {
            index: 2,
            id: Some("tc_3".into()),
            kind: None,
            function: Some(DeltaFunction {
                name: Some("grep".into()),
                arguments: None,
            }),
        };
        a.apply_tool_call_delta(&delta);
        assert_eq!(a.tool_calls.len(), 3);
        assert_eq!(a.tool_calls[2].function.name, "grep");
        // Filler entries have empty values
        assert!(a.tool_calls[0].id.is_empty());
        assert!(a.tool_calls[1].id.is_empty());
    }

    #[test]
    fn delta_appends_arguments_incrementally() {
        let mut a = StreamAssembler::new();
        let d1 = DeltaToolCall {
            index: 0,
            id: Some("tc_1".into()),
            kind: None,
            function: Some(DeltaFunction {
                name: Some("edit_file".into()),
                arguments: Some(r#"{"path":"#.into()),
            }),
        };
        let d2 = DeltaToolCall {
            index: 0,
            id: None,
            kind: None,
            function: Some(DeltaFunction {
                name: None,
                arguments: Some(r#""x.rs"}"#.into()),
            }),
        };
        a.apply_tool_call_delta(&d1);
        a.apply_tool_call_delta(&d2);
        assert_eq!(a.tool_calls[0].function.arguments, r#"{"path":"x.rs"}"#);
    }

    #[test]
    fn delta_with_all_none_fields() {
        let mut a = StreamAssembler::new();
        // First, create an entry
        a.tool_calls.push(ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
        });
        let delta = DeltaToolCall {
            index: 0,
            id: None,
            kind: None,
            function: None,
        };
        a.apply_tool_call_delta(&delta);
        // Nothing should change
        assert_eq!(a.tool_calls[0].id, "tc_1");
        assert_eq!(a.tool_calls[0].function.name, "bash");
    }

    // ── LlmClient construction tests ──

    #[test]
    fn client_strips_trailing_slash_from_url() {
        let c = LlmClient::new("https://api.example.com/v1/", "key", "model");
        assert_eq!(c.base_url, "https://api.example.com/v1");
    }

    #[test]
    fn client_preserves_url_without_trailing_slash() {
        let c = LlmClient::new("https://api.example.com/v1", "key", "model");
        assert_eq!(c.base_url, "https://api.example.com/v1");
    }

    #[test]
    fn client_stores_api_key_and_model() {
        let c = LlmClient::new("http://localhost:11434/v1", "test-key", "llama3");
        assert_eq!(c.api_key, "test-key");
        assert_eq!(c.model, "llama3");
    }

    // ── finish() filtering and fallback ID tests ──

    #[test]
    fn finish_filters_out_filler_entries_with_empty_name() {
        let mut a = StreamAssembler::new();
        // Simulate sparse index: delta at index 2 creates fillers at 0 and 1
        let delta = DeltaToolCall {
            index: 2,
            id: Some("tc_3".into()),
            kind: None,
            function: Some(DeltaFunction {
                name: Some("grep".into()),
                arguments: Some("{}".into()),
            }),
        };
        a.apply_tool_call_delta(&delta);
        assert_eq!(a.tool_calls.len(), 3);

        let (msg, _usage) = a.finish();
        let tcs = msg.tool_calls.unwrap();
        // Only the valid entry should remain
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].function.name, "grep");
        assert_eq!(tcs[0].id, "tc_3");
    }

    #[test]
    fn finish_generates_fallback_id_when_missing() {
        let mut a = StreamAssembler::new();
        // Some providers don't include tool call IDs in streaming
        a.tool_calls.push(ToolCall {
            id: String::new(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: r#"{"path":"test.rs"}"#.into(),
            },
        });
        let (msg, _usage) = a.finish();
        let tcs = msg.tool_calls.unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_0");
        assert_eq!(tcs[0].function.name, "read_file");
    }

    #[test]
    fn finish_preserves_existing_ids() {
        let mut a = StreamAssembler::new();
        a.tool_calls.push(ToolCall {
            id: "real_id_123".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
        });
        let (msg, _usage) = a.finish();
        let tcs = msg.tool_calls.unwrap();
        assert_eq!(tcs[0].id, "real_id_123");
    }

    #[test]
    fn finish_filters_all_empty_returns_none() {
        let mut a = StreamAssembler::new();
        // Only filler entries, no real tool calls
        a.tool_calls.push(ToolCall {
            id: String::new(),
            kind: "function".into(),
            function: FunctionCall {
                name: String::new(),
                arguments: String::new(),
            },
        });
        let (msg, _usage) = a.finish();
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn finish_mixed_valid_and_filler() {
        let mut a = StreamAssembler::new();
        a.tool_calls.push(ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: "{}".into(),
            },
        });
        // Filler
        a.tool_calls.push(ToolCall {
            id: String::new(),
            kind: "function".into(),
            function: FunctionCall {
                name: String::new(),
                arguments: String::new(),
            },
        });
        a.tool_calls.push(ToolCall {
            id: "tc_3".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
        });
        let (msg, _usage) = a.finish();
        let tcs = msg.tool_calls.unwrap();
        assert_eq!(tcs.len(), 2);
        assert_eq!(tcs[0].function.name, "read_file");
        assert_eq!(tcs[1].function.name, "bash");
    }

    #[test]
    fn finish_generates_sequential_fallback_ids() {
        let mut a = StreamAssembler::new();
        for i in 0..3 {
            a.tool_calls.push(ToolCall {
                id: String::new(),
                kind: "function".into(),
                function: FunctionCall {
                    name: format!("tool_{i}"),
                    arguments: "{}".into(),
                },
            });
        }
        let (msg, _usage) = a.finish();
        let tcs = msg.tool_calls.unwrap();
        assert_eq!(tcs[0].id, "call_0");
        assert_eq!(tcs[1].id, "call_1");
        assert_eq!(tcs[2].id, "call_2");
    }
}
