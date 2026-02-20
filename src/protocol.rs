//! MCP Protocol types - JSON-RPC 2.0 messages for Model Context Protocol.
//! We implement the protocol directly (no SDK) for smallest binary and fastest startup.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── JSON-RPC 2.0 Base Types ─────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
    }

    pub fn error(id: Option<Value>, code: i64, message: String) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: None, error: Some(JsonRpcError { code, message }) }
    }

    #[allow(dead_code)]
    pub fn notification(method: &str, params: Value) -> String {
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        })).unwrap()
    }
}

// ─── MCP Tool Types ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct ToolContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

#[allow(dead_code)]
impl ToolContent {
    pub fn text(s: impl Into<String>) -> Self {
        Self { content_type: "text".into(), text: s.into() }
    }
}

// ─── MCP Initialize Types ────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: Capabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Capabilities {
    pub tools: ToolsCapability,
    pub prompts: PromptsCapability,
    pub resources: ResourcesCapability,
}

#[derive(Debug, Serialize)]
pub struct ToolsCapability {}

#[derive(Debug, Serialize)]
pub struct PromptsCapability {}

#[derive(Debug, Serialize)]
pub struct ResourcesCapability {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_jsonrpc_request_parsing() {
        let req_str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let req: JsonRpcRequest = serde_json::from_str(req_str).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(json!(1)));
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.params, json!(null));
    }

    #[test]
    fn test_jsonrpc_response_success() {
        let resp = JsonRpcResponse::success(Some(json!(1)), json!({"status": "ok"}));
        let resp_str = serde_json::to_string(&resp).unwrap();
        assert!(resp_str.contains(r#""jsonrpc":"2.0""#));
        assert!(resp_str.contains(r#""id":1"#));
        assert!(resp_str.contains(r#""result":{"status":"ok"}"#));
        assert!(!resp_str.contains("error"));
    }

    #[test]
    fn test_jsonrpc_response_error() {
        let resp = JsonRpcResponse::error(Some(json!(2)), -32601, "Method not found".to_string());
        let resp_str = serde_json::to_string(&resp).unwrap();
        assert!(resp_str.contains(r#""jsonrpc":"2.0""#));
        assert!(resp_str.contains(r#""id":2"#));
        assert!(resp_str.contains(r#""error":{"code":-32601,"message":"Method not found"}"#));
        assert!(!resp_str.contains("result"));
    }

    #[test]
    fn test_capabilities_serialization() {
        let init_result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: Capabilities {
                tools: ToolsCapability {},
                prompts: PromptsCapability {},
                resources: ResourcesCapability {},
            },
            server_info: ServerInfo {
                name: "McpHub".to_string(),
                version: "2.0.0".to_string(),
            },
            instructions: None,
        };

        let result_str = serde_json::to_string(&init_result).unwrap();
        assert!(result_str.contains(r#""capabilities":{"tools":{},"prompts":{},"resources":{}}"#));
        assert!(result_str.contains(r#""serverInfo":{"name":"McpHub","version":"2.0.0"}"#));
    }
}
