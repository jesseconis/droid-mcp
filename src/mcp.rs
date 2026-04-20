use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ── JSON-RPC 2.0 ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
    }

    pub fn error(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
        }
    }

    pub fn method_not_found(id: Option<Value>) -> Self {
        Self::error(id, -32601, "Method not found")
    }
}

// ── MCP types ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ServerCapabilities {
    pub tools: Option<ToolsCapability>,
}

#[derive(Debug, Serialize)]
pub struct ToolsCapability {
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: InputSchema,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub properties: HashMap<String, PropertySchema>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PropertySchema {
    #[serde(rename = "type")]
    pub prop_type: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct ToolResult {
    pub content: Vec<ToolResultContent>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ToolResultContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

impl ToolResult {
    pub fn text(output: impl Into<String>) -> Self {
        Self {
            content: vec![ToolResultContent {
                content_type: "text".into(),
                text: output.into(),
            }],
            is_error: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            content: vec![ToolResultContent {
                content_type: "text".into(),
                text: msg.into(),
            }],
            is_error: Some(true),
        }
    }
}

// ── MCP protocol version ───────────────────────────────────────────────

pub const PROTOCOL_VERSION: &str = "2024-11-05";
pub const SERVER_NAME: &str = "droid-mcp";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
