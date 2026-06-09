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

    /// Number of connected subscribers (used to gate the state-event tick timer:
    /// it only does work when telemetry is on AND someone is listening, DW3).
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
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

/// Handle the state-event WebSocket upgrade (`/ws/state`, Sprint W7). Unlike the
/// log stream, frames here are already typed JSON (tick frames with positions +
/// meters, and discrete state events) produced by the control thread, so they are
/// forwarded verbatim.
pub async fn handle_state_websocket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_state_socket(socket, state.state_broadcaster))
}

async fn handle_state_socket(socket: WebSocket, broadcaster: Arc<LogBroadcaster>) {
    let (mut sender, mut receiver) = socket.split();
    let mut state_rx = broadcaster.subscribe();

    let send_task = tokio::spawn(async move {
        loop {
            match state_rx.recv().await {
                Ok(frame) => {
                    if sender.send(Message::Text(frame)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::trace!("state WebSocket client lagged, missed {} frames", n);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    while let Some(result) = receiver.next().await {
        match result {
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("state WebSocket error: {}", e);
                break;
            }
        }
    }

    send_task.abort();
}

/// A tracing layer that sends log messages to the WebSocket broadcaster, so
/// `/ws` clients stream the daemon's live log lines (D62). `main` installs it in
/// the tracing registry alongside the fmt/MQTT layers, sharing the broadcaster
/// the HTTP server serves `/ws` from. The layer must never emit tracing events
/// itself while broadcasting (it would recurse into the subscriber).
pub struct WebSocketLogLayer {
    broadcaster: Arc<LogBroadcaster>,
}

impl WebSocketLogLayer {
    /// Create the layer over the broadcaster `/ws` clients subscribe to.
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
    use std::sync::Mutex;
    use tracing_subscriber::layer::SubscriberExt;

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

    /// A layer that runs `LogVisitor` against each event and records the extracted
    /// message, so the (private) visitor can be exercised through real tracing
    /// events without hand-constructing an `Event`.
    struct VisitorProbe {
        messages: Arc<Mutex<Vec<String>>>,
    }

    impl<S> tracing_subscriber::Layer<S> for VisitorProbe
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = LogVisitor::default();
            event.record(&mut visitor);
            self.messages.lock().unwrap().push(visitor.message);
        }
    }

    #[test]
    fn test_log_visitor_extracts_message_field() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(VisitorProbe {
            messages: messages.clone(),
        });

        tracing::subscriber::with_default(subscriber, || {
            // A plain log message is recorded as the `message` field.
            tracing::info!("hello world");
        });

        let messages = messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0], "hello world");
    }

    #[test]
    fn test_log_visitor_message_field_wins_over_other_fields() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(VisitorProbe {
            messages: messages.clone(),
        });

        tracing::subscriber::with_default(subscriber, || {
            // The `message` field must win even when a structured field is recorded
            // first (LogVisitor overwrites whatever the first field set once it sees
            // a field literally named "message").
            tracing::info!(count = 7, "the real message");
        });

        let messages = messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0], "the real message");
    }

    #[test]
    fn test_log_visitor_captures_non_message_first_field() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(VisitorProbe {
            messages: messages.clone(),
        });

        tracing::subscriber::with_default(subscriber, || {
            // No message string: the first (and only) field is not named "message",
            // so LogVisitor falls back to capturing it because `message` is empty.
            tracing::info!(detail = "first-field-value");
        });

        let messages = messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0], "first-field-value",
            "with no message field, the first field's value is captured"
        );
    }

    #[test]
    fn test_websocket_log_layer_on_event_format_and_broadcast() {
        let broadcaster = Arc::new(LogBroadcaster::new());
        let mut rx = broadcaster.subscribe();

        let layer = WebSocketLogLayer::new(broadcaster.clone());
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("disk almost full");
        });

        let line = rx.try_recv().expect("on_event should broadcast a line");
        // Format is "<timestamp> [<LEVEL>] <target>: <message>".
        assert!(
            line.contains("[WARN]"),
            "line should carry the level, got: {line}"
        );
        assert!(
            line.contains("disk almost full"),
            "line should carry the message, got: {line}"
        );
        // The target is the module path of the emitting code; the formatted line
        // ends with ": <message>" after the target.
        assert!(
            line.contains(": disk almost full"),
            "line should separate target and message with ': ', got: {line}"
        );
        // A second event broadcasts a second distinct line.
        let subscriber2 =
            tracing_subscriber::registry().with(WebSocketLogLayer::new(broadcaster.clone()));
        tracing::subscriber::with_default(subscriber2, || {
            tracing::info!("back to normal");
        });
        let line2 = rx.try_recv().expect("second event should broadcast");
        assert!(line2.contains("[INFO]") && line2.contains("back to normal"));
    }
}
