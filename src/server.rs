use crate::mcp::*;
use crate::tools::ToolRegistry;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct McpServer {
    registry: Arc<ToolRegistry>,
}

impl McpServer {
    pub fn new(registry: ToolRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
        }
    }

    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    /// Handle a single JSON-RPC request and return a response.
    /// Returns None for notifications (no id).
    pub async fn handle(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        // Notifications have no id — don't respond
        if req.id.is_none() {
            tracing::debug!(method = %req.method, "received notification");
            return None;
        }

        let id = req.id.clone();

        let response = match req.method.as_str() {
            "initialize" => self.handle_initialize(id, req.params).await,
            "tools/list" => self.handle_tools_list(id).await,
            "tools/call" => self.handle_tools_call(id, req.params).await,
            "ping" => JsonRpcResponse::success(id, json!({})),
            _ => {
                tracing::warn!(method = %req.method, "unknown method");
                JsonRpcResponse::method_not_found(id)
            }
        };

        Some(response)
    }

    async fn handle_initialize(
        &self,
        id: Option<Value>,
        _params: Option<Value>,
    ) -> JsonRpcResponse {
        tracing::info!("client initializing");

        JsonRpcResponse::success(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {
                        "listChanged": false
                    }
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION
                }
            }),
        )
    }

    async fn handle_tools_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let tools = self.registry.definitions();
        JsonRpcResponse::success(id, json!({ "tools": tools }))
    }

    async fn handle_tools_call(
        &self,
        id: Option<Value>,
        params: Option<Value>,
    ) -> JsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => {
                return JsonRpcResponse::error(id, -32602, "missing params");
            }
        };

        let tool_name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => {
                return JsonRpcResponse::error(id, -32602, "missing tool name");
            }
        };

        let arguments = params
            .get("arguments")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        tracing::info!(tool = %tool_name, "calling tool");

        let result = self.registry.call(&tool_name, &arguments).await;

        JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
    }
}
