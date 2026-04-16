use serde::{Deserialize, Serialize};

// ── Request types ──

#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Option<Vec<ToolCall>>) -> Self {
        Self {
            role: "assistant".into(),
            content,
            tool_calls,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

// ── Tool definitions ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionDef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// ── Tool calls ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

// ── Streaming response types ──

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: Delta,
    #[allow(dead_code)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Delta {
    #[allow(dead_code)]
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

#[derive(Debug, Deserialize)]
pub struct DeltaToolCall {
    pub index: usize,
    pub id: Option<String>,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub kind: Option<String>,
    pub function: Option<DeltaFunction>,
}

#[derive(Debug, Deserialize)]
pub struct DeltaFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

// ── Non-streaming response (fallback) ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Choice {
    pub message: Message,
    pub finish_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Message constructors ──

    #[test]
    fn message_system_has_correct_role_and_content() {
        let m = Message::system("You are helpful.");
        assert_eq!(m.role, "system");
        assert_eq!(m.content.unwrap(), "You are helpful.");
        assert!(m.tool_calls.is_none());
        assert!(m.tool_call_id.is_none());
    }

    #[test]
    fn message_user_has_correct_role_and_content() {
        let m = Message::user("Hello");
        assert_eq!(m.role, "user");
        assert_eq!(m.content.unwrap(), "Hello");
        assert!(m.tool_calls.is_none());
        assert!(m.tool_call_id.is_none());
    }

    #[test]
    fn message_assistant_with_content_only() {
        let m = Message::assistant(Some("Hi there".into()), None);
        assert_eq!(m.role, "assistant");
        assert_eq!(m.content.unwrap(), "Hi there");
        assert!(m.tool_calls.is_none());
    }

    #[test]
    fn message_assistant_with_tool_calls_only() {
        let tc = vec![ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: r#"{"path":"x.rs"}"#.into(),
            },
        }];
        let m = Message::assistant(None, Some(tc));
        assert_eq!(m.role, "assistant");
        assert!(m.content.is_none());
        assert_eq!(m.tool_calls.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn message_assistant_with_both_content_and_tool_calls() {
        let tc = vec![ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: r#"{"command":"ls"}"#.into(),
            },
        }];
        let m = Message::assistant(Some("Let me check.".into()), Some(tc));
        assert!(m.content.is_some());
        assert!(m.tool_calls.is_some());
    }

    #[test]
    fn message_tool_result_has_correct_fields() {
        let m = Message::tool_result("call_abc", "file contents here");
        assert_eq!(m.role, "tool");
        assert_eq!(m.content.unwrap(), "file contents here");
        assert_eq!(m.tool_call_id.unwrap(), "call_abc");
        assert!(m.tool_calls.is_none());
    }

    // ── Serialization tests ──

    #[test]
    fn message_serializes_skipping_none_fields() {
        let m = Message::user("hi");
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "hi");
        // None fields should be absent, not null
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("tool_call_id").is_none());
    }

    #[test]
    fn message_serializes_tool_call_id_when_present() {
        let m = Message::tool_result("tc_1", "result");
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["tool_call_id"], "tc_1");
    }

    #[test]
    fn tool_def_serializes_type_as_type_key() {
        let td = ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "test".into(),
                description: "desc".into(),
                parameters: json!({"type": "object"}),
            },
        };
        let json = serde_json::to_value(&td).unwrap();
        assert_eq!(json["type"], "function");
        // "kind" should NOT appear
        assert!(json.get("kind").is_none());
    }

    #[test]
    fn tool_call_serializes_type_as_type_key() {
        let tc = ToolCall {
            id: "tc_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
        };
        let json = serde_json::to_value(&tc).unwrap();
        assert_eq!(json["type"], "function");
        assert!(json.get("kind").is_none());
        assert_eq!(json["id"], "tc_1");
    }

    #[test]
    fn chat_request_serializes_without_optional_fields() {
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![Message::user("hi")],
            tools: None,
            tool_choice: None,
            stream: true,
            temperature: None,
            max_tokens: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "gpt-4o");
        assert_eq!(json["stream"], true);
        assert!(json.get("tools").is_none());
        assert!(json.get("tool_choice").is_none());
        assert!(json.get("temperature").is_none());
        assert!(json.get("max_tokens").is_none());
    }

    #[test]
    fn chat_request_serializes_with_all_fields() {
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![Message::user("hi")],
            tools: Some(vec![]),
            tool_choice: Some(json!("auto")),
            stream: false,
            temperature: Some(0.7),
            max_tokens: Some(1024),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!((json["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
        assert_eq!(json["max_tokens"], 1024);
        assert!(json["tools"].is_array());
        assert_eq!(json["tool_choice"], "auto");
    }

    // ── Deserialization tests ──

    #[test]
    fn message_deserializes_from_api_response() {
        let json = r#"{
            "role": "assistant",
            "content": "Hello!",
            "tool_calls": null
        }"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, "assistant");
        assert_eq!(msg.content.unwrap(), "Hello!");
    }

    #[test]
    fn message_deserializes_with_tool_calls() {
        let json = r#"{
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": "call_123",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": "{\"path\":\"main.rs\"}"
                }
            }]
        }"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(msg.content.is_none());
        let tcs = msg.tool_calls.unwrap();
        assert_eq!(tcs[0].id, "call_123");
        assert_eq!(tcs[0].kind, "function");
        assert_eq!(tcs[0].function.name, "read_file");
    }

    #[test]
    fn stream_chunk_deserializes_content_delta() {
        let json = r#"{"choices":[{"delta":{"content":"tok"},"finish_reason":null}]}"#;
        let chunk: StreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content.as_ref().unwrap(), "tok");
    }

    #[test]
    fn stream_chunk_deserializes_tool_call_delta() {
        let json = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"bash","arguments":"{}"}}]},"finish_reason":null}]}"#;
        let chunk: StreamChunk = serde_json::from_str(json).unwrap();
        let tc = &chunk.choices[0].delta.tool_calls.as_ref().unwrap()[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id.as_ref().unwrap(), "c1");
        assert_eq!(tc.function.as_ref().unwrap().name.as_ref().unwrap(), "bash");
    }

    #[test]
    fn stream_chunk_deserializes_finish_reason() {
        let json = r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let chunk: StreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].finish_reason.as_ref().unwrap(), "stop");
    }

    #[test]
    fn chat_response_deserializes() {
        let json = r#"{
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }]
        }"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.role, "assistant");
        assert_eq!(resp.choices[0].finish_reason.as_ref().unwrap(), "stop");
    }

    #[test]
    fn tool_call_round_trips_through_serde() {
        let tc = ToolCall {
            id: "tc_99".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "edit_file".into(),
                arguments: r#"{"path":"a.rs","old_string":"x","new_string":"y"}"#.into(),
            },
        };
        let json = serde_json::to_string(&tc).unwrap();
        let tc2: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(tc2.id, "tc_99");
        assert_eq!(tc2.kind, "function");
        assert_eq!(tc2.function.name, "edit_file");
        assert_eq!(tc2.function.arguments, tc.function.arguments);
    }

    #[test]
    fn delta_tool_call_deserializes_partial_fields() {
        // Only arguments, no id/name (continuation delta)
        let json = r#"{"index":0,"function":{"arguments":"more args"}}"#;
        let dtc: DeltaToolCall = serde_json::from_str(json).unwrap();
        assert_eq!(dtc.index, 0);
        assert!(dtc.id.is_none());
        assert!(dtc.kind.is_none());
        assert_eq!(
            dtc.function.as_ref().unwrap().arguments.as_ref().unwrap(),
            "more args"
        );
        assert!(dtc.function.as_ref().unwrap().name.is_none());
    }
}
