use crate::mcp::JsonRpcRequest;
use crate::server::McpServer;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn serve_stdio(server: McpServer) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("stdio transport ready");

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse JSON-RPC");
                let err = crate::mcp::JsonRpcResponse::error(
                    None,
                    -32700,
                    format!("parse error: {}", e),
                );
                let out = serde_json::to_string(&err).unwrap();
                stdout.write_all(out.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
                continue;
            }
        };

        if let Some(response) = server.handle(req).await {
            let out = serde_json::to_string(&response).unwrap();
            stdout.write_all(out.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }

    Ok(())
}
