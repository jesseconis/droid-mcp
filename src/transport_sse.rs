use crate::mcp::JsonRpcRequest;
use crate::server::McpServer;
use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Json, Router,
};
use dashmap::DashMap;
use serde_json::Value;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

type SseEvent = Result<Event, Infallible>;
type SseChannels = Arc<DashMap<String, mpsc::Sender<SseEvent>>>;

#[derive(Clone)]
struct AppState {
    server: Arc<McpServer>,
    channels: SseChannels,
}

pub async fn serve_sse(server: McpServer, bind_addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let state = AppState {
        server: Arc::new(server),
        channels: Arc::new(DashMap::new()),
    };

    let app = Router::new()
        .route("/sse", get(sse_handler))
        .route("/message", post(message_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(addr = %bind_addr, "SSE transport listening");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<ReceiverStream<SseEvent>> {
    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel::<SseEvent>(32);

    // Send the endpoint event so the client knows where to POST
    let endpoint_uri = format!("/message?sessionId={}", session_id);
    let _ = tx
        .send(Ok(
            Event::default()
                .event("endpoint")
                .data(endpoint_uri),
        ))
        .await;

    state.channels.insert(session_id.clone(), tx);
    tracing::info!(session = %session_id, "SSE client connected");

    // Clean up when the stream drops
    let channels = state.channels.clone();
    let sid = session_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        channels.remove(&sid);
    });

    Sse::new(ReceiverStream::new(rx))
}

async fn message_handler(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let session_id = match query.get("sessionId") {
        Some(id) => id.clone(),
        None => return (StatusCode::BAD_REQUEST, "missing sessionId").into_response(),
    };

    let tx = match state.channels.get(&session_id) {
        Some(tx) => tx.clone(),
        None => return (StatusCode::NOT_FOUND, "unknown session").into_response(),
    };

    let req: JsonRpcRequest = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid request: {}", e)).into_response();
        }
    };

    let server = state.server.clone();

    tokio::spawn(async move {
        if let Some(response) = server.handle(req).await {
            let data = serde_json::to_string(&response).unwrap();
            let event: SseEvent = Ok(Event::default().event("message").data(data));
            let _ = tx.send(event).await;
        }
    });

    StatusCode::ACCEPTED.into_response()
}
