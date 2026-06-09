// ABOUTME: MQTT log publisher for forwarding logs to an MQTT topic.
// ABOUTME: Uses a channel-based approach for async log publishing.

use rumqttc::{AsyncClient, QoS};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Log entry published to MQTT
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// Sender side of the log channel
pub type LogSender = mpsc::Sender<LogEntry>;

/// Receiver side of the log channel
pub type LogReceiver = mpsc::Receiver<LogEntry>;

/// Create a log channel for publishing logs to MQTT
pub fn create_log_channel(buffer_size: usize) -> (LogSender, LogReceiver) {
    mpsc::channel(buffer_size)
}

/// Tracing layer that forwards log events to a channel
pub struct MqttLogLayer {
    sender: LogSender,
    min_level: Level,
}

impl MqttLogLayer {
    /// Create a new MQTT log layer
    pub fn new(sender: LogSender, min_level: Level) -> Self {
        Self { sender, min_level }
    }
}

impl<S> Layer<S> for MqttLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // Check if we should log this level
        if event.metadata().level() > &self.min_level {
            return;
        }

        // Extract message from the event
        let mut message = String::new();
        let mut visitor = MessageVisitor {
            message: &mut message,
        };
        event.record(&mut visitor);

        let entry = LogEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            level: event.metadata().level().to_string(),
            target: event.metadata().target().to_string(),
            message,
        };

        // Try to send, but don't block if channel is full
        let _ = self.sender.try_send(entry);
    }
}

/// Visitor to extract message from tracing event
struct MessageVisitor<'a> {
    message: &'a mut String,
}

impl<'a> tracing::field::Visit for MessageVisitor<'a> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.message = format!("{:?}", value);
        } else if self.message.is_empty() {
            // If no message field, use the first field's value
            *self.message = format!("{:?}", value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" || self.message.is_empty() {
            *self.message = value.to_string();
        }
    }
}

/// Spawn a task that publishes log entries to MQTT
pub fn spawn_log_publisher(
    client: Arc<AsyncClient>,
    topic: String,
    mut receiver: LogReceiver,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(entry) = receiver.recv().await {
            // Serialize to JSON
            let json = match serde_json::to_string(&entry) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("Failed to serialize log entry: {}", e);
                    continue;
                }
            };

            // Publish to MQTT (fire and forget)
            if let Err(e) = client
                .publish(&topic, QoS::AtMostOnce, false, json.as_bytes())
                .await
            {
                // Only print if it's not a channel closed error
                if !matches!(e, rumqttc::ClientError::Request(_)) {
                    eprintln!("Failed to publish log to MQTT: {}", e);
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_entry_serialization() {
        let entry = LogEntry {
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            level: "INFO".to_string(),
            target: "mqttaudio".to_string(),
            message: "Test message".to_string(),
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"level\":\"INFO\""));
        assert!(json.contains("\"message\":\"Test message\""));
    }

    #[tokio::test]
    async fn test_log_channel() {
        let (sender, mut receiver) = create_log_channel(10);

        let entry = LogEntry {
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            level: "INFO".to_string(),
            target: "test".to_string(),
            message: "Hello".to_string(),
        };

        sender.send(entry.clone()).await.unwrap();

        let received = receiver.recv().await.unwrap();
        assert_eq!(received.message, "Hello");
        assert_eq!(received.level, "INFO");
    }
}
