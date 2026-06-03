// ABOUTME: WebSocket endpoint for real-time log streaming.
// ABOUTME: Broadcasts log messages to connected clients for admin monitoring.

use super::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Maximum number of log messages to buffer for new subscribers.
const LOG_BUFFER_SIZE: usize = 1000;

/// Broadcasts log messages to connected WebSocket clients.
pub struct LogBroadcaster {
    sender: broadcast::Sender<String>,
}

impl LogBroadcaster {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(LOG_BUFFER_SIZE);
        Self { sender }
    }

    /// Send a log message to all connected clients.
    pub fn broadcast(&self, message: String) {
        // Ignore errors - no subscribers is fine
        let _ = self.sender.send(message);
    }

    /// Subscribe to log messages.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.sender.subscribe()
    }
}

impl Default for LogBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle WebSocket upgrade request.
pub async fn handle_websocket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state.log_broadcaster))
}

/// Handle an individual WebSocket connection.
async fn handle_socket(socket: WebSocket, broadcaster: Arc<LogBroadcaster>) {
    let (mut sender, mut receiver) = socket.split();
    let mut log_rx = broadcaster.subscribe();

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "connected",
        "message": "Connected to mqttaudio log stream",
        "version": env!("CARGO_PKG_VERSION")
    });

    if sender
        .send(Message::Text(welcome.to_string()))
        .await
        .is_err()
    {
        return;
    }

    // Spawn a task to forward log messages to the client
    let send_task = tokio::spawn(async move {
        loop {
            match log_rx.recv().await {
                Ok(msg) => {
                    let log_message = serde_json::json!({
                        "type": "log",
                        "message": msg
                    });

                    if sender
                        .send(Message::Text(log_message.to_string()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("WebSocket client lagged, missed {} messages", n);
                    // Continue receiving
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    });

    // Handle incoming messages from client (mostly for keep-alive pings)
    while let Some(result) = receiver.next().await {
        match result {
            Ok(Message::Ping(data)) => {
                // Axum handles pong automatically
                tracing::trace!("WebSocket ping received: {:?}", data);
            }
            Ok(Message::Close(_)) => {
                break;
            }
            Ok(_) => {
                // Ignore other message types
            }
            Err(e) => {
                tracing::debug!("WebSocket error: {}", e);
                break;
            }
        }
    }

    // Clean up
    send_task.abort();
}

/// A tracing layer that sends log messages to the WebSocket broadcaster.
/// This integrates with the existing tracing infrastructure.
pub struct WebSocketLogLayer {
    broadcaster: Arc<LogBroadcaster>,
}

impl WebSocketLogLayer {
    /// Create a new WebSocket log layer for tracing integration.
    /// Note: This is designed for future integration with tracing-subscriber.
    #[allow(dead_code)]
    pub fn new(broadcaster: Arc<LogBroadcaster>) -> Self {
        Self { broadcaster }
    }
}

impl<S> tracing_subscriber::Layer<S> for WebSocketLogLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Format the event as a simple message
        let mut visitor = LogVisitor::default();
        event.record(&mut visitor);

        let level = event.metadata().level();
        let target = event.metadata().target();

        let log_line = format!(
            "{} [{}] {}: {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            level,
            target,
            visitor.message
        );

        self.broadcaster.broadcast(log_line);
    }
}

#[derive(Default)]
struct LogVisitor {
    message: String,
}

impl tracing::field::Visit for LogVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" || self.message.is_empty() {
            self.message = format!("{:?}", value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" || self.message.is_empty() {
            self.message = value.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_broadcaster_creation() {
        let broadcaster = LogBroadcaster::new();
        // Should not panic
        broadcaster.broadcast("test message".to_string());
    }

    #[test]
    fn test_log_broadcaster_with_subscriber() {
        let broadcaster = LogBroadcaster::new();
        let mut rx = broadcaster.subscribe();

        broadcaster.broadcast("test message".to_string());

        // Use try_recv since there's no async context
        match rx.try_recv() {
            Ok(msg) => assert_eq!(msg, "test message"),
            Err(_) => panic!("Should have received message"),
        }
    }
}
