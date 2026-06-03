// ABOUTME: MQTT connection management using rumqttc.
// ABOUTME: Handles connection, reconnection, and message receiving.

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Publish, QoS};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum MqttError {
    SubscriptionError(rumqttc::ClientError),
}

impl std::fmt::Display for MqttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MqttError::SubscriptionError(e) => write!(f, "MQTT subscription error: {}", e),
        }
    }
}

impl std::error::Error for MqttError {}

/// Connect to MQTT broker and subscribe to topic
pub async fn connect_mqtt(
    server: &str,
    port: u16,
    topic: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<(AsyncClient, EventLoop), MqttError> {
    tracing::info!("Connecting to MQTT broker: {}:{}", server, port);

    // Use unique client ID with random suffix to avoid conflicts
    use rand::Rng;
    let random_suffix: u32 = rand::thread_rng().gen();
    let client_id = format!("mqttaudio_{:08x}", random_suffix);

    let mut mqttoptions = MqttOptions::new(client_id, server, port);
    mqttoptions.set_keep_alive(Duration::from_secs(60));
    mqttoptions.set_clean_session(true);

    // Set credentials if provided
    if let (Some(user), Some(pass)) = (username, password) {
        tracing::info!("Using MQTT authentication for user: {}", user);
        mqttoptions.set_credentials(user, pass);
    }

    let (client, eventloop) = AsyncClient::new(mqttoptions, 10);

    // Subscribe to command topic
    client
        .subscribe(topic, QoS::AtLeastOnce)
        .await
        .map_err(MqttError::SubscriptionError)?;

    tracing::info!("Subscribed to topic: {}", topic);

    Ok((client, eventloop))
}

/// Extract the command payload from a Publish packet as lossy UTF-8.
/// Pure and broker-independent so it can be unit-tested directly.
pub fn publish_payload(publish: &Publish) -> String {
    String::from_utf8_lossy(&publish.payload).to_string()
}

/// Process MQTT events and forward messages to command channel.
/// Re-subscribes to `topic` on every `ConnAck` so a broker restart (with
/// `clean_session`) does not leave the daemon silently unsubscribed.
pub async fn process_mqtt_events(
    client: AsyncClient,
    topic: String,
    mut eventloop: EventLoop,
    command_tx: mpsc::Sender<String>,
) {
    tracing::info!("Starting MQTT event loop");

    let mut dropped_commands: u64 = 0;
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let payload = publish_payload(&p);
                tracing::debug!("Received MQTT message on topic {}: {}", p.topic, payload);

                // Never block the MQTT event loop on a slow consumer: drop (with a
                // warning + running count) rather than back-pressuring poll(), which
                // would stall keepalive and get the broker to drop the session.
                match command_tx.try_send(payload) {
                    Ok(()) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        dropped_commands += 1;
                        tracing::warn!(
                            "Command queue full; dropped MQTT command (total dropped: {})",
                            dropped_commands
                        );
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        tracing::error!("Command channel closed; stopping MQTT event loop");
                        return;
                    }
                }
            }
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                tracing::info!("MQTT connected");
                // Re-subscribe on every (re)connect; the broker keeps no
                // subscription across a clean_session reconnect.
                if let Err(e) = client.subscribe(&topic, QoS::AtLeastOnce).await {
                    tracing::error!("Failed to (re)subscribe to '{}': {}", topic, e);
                }
            }
            Ok(Event::Incoming(Packet::SubAck(_))) => {
                tracing::debug!("MQTT subscription acknowledged");
            }
            Ok(event) => {
                // Other events (PingResp, etc.)
                tracing::trace!("MQTT event: {:?}", event);
            }
            Err(e) => {
                tracing::error!("MQTT error: {}", e);
                // Wait before reconnecting
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live-broker tests run only when `MQTTAUDIO_BROKER_TESTS` is set (Lane A
    /// sets it with a containerized mosquitto). A bare `cargo test` skips them
    /// so it never depends on a running broker.
    fn broker_tests_enabled() -> bool {
        std::env::var("MQTTAUDIO_BROKER_TESTS").is_ok()
    }

    #[test]
    fn publish_payload_decodes_lossy_utf8() {
        let valid = Publish::new("topic", QoS::AtLeastOnce, b"hello".to_vec());
        assert_eq!(publish_payload(&valid), "hello");

        // Invalid UTF-8 must not panic; it decodes to the replacement character.
        let invalid = Publish::new("topic", QoS::AtLeastOnce, vec![0xff, 0xfe]);
        assert!(publish_payload(&invalid).contains('\u{FFFD}'));
    }

    #[tokio::test]
    async fn test_connect_mqtt() {
        if !broker_tests_enabled() {
            return;
        }
        let result = connect_mqtt("localhost", 1883, "test/topic", None, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_connect_mqtt_with_credentials() {
        if !broker_tests_enabled() {
            return;
        }
        // Test that credentials are accepted (actual authentication requires a configured broker)
        let result =
            connect_mqtt("localhost", 1883, "test/topic", Some("user"), Some("pass")).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mqtt_event_processing() {
        if !broker_tests_enabled() {
            return;
        }
        let (client, eventloop) = connect_mqtt("localhost", 1883, "test/topic", None, None)
            .await
            .unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        // Spawn event processor with its own client clone for resubscribe.
        let proc_client = client.clone();
        tokio::spawn(async move {
            process_mqtt_events(proc_client, "test/topic".to_string(), eventloop, tx).await;
        });

        // Publish a test message
        client
            .publish(
                "test/topic",
                QoS::AtLeastOnce,
                false,
                r#"{"command":"play","message":{"file":"test.wav"}}"#,
            )
            .await
            .unwrap();

        // Receive the message
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Timeout waiting for message")
            .expect("Channel closed");
    }
}
