// ABOUTME: MQTT connection management using rumqttc.
// ABOUTME: Handles connection, reconnection, and message receiving.

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
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
    client_id: Option<&str>,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<(AsyncClient, EventLoop), MqttError> {
    tracing::info!("Connecting to MQTT broker: {}:{}", server, port);

    // Use the configured client ID, or a unique random one to avoid conflicts
    let client_id = match client_id {
        Some(id) => id.to_string(),
        None => {
            use rand::Rng;
            let random_suffix: u32 = rand::thread_rng().gen();
            format!("mqttaudio_{:08x}", random_suffix)
        }
    };

    let mut mqttoptions = MqttOptions::new(client_id, server, port);
    mqttoptions.set_keep_alive(Duration::from_secs(60));
    mqttoptions.set_clean_session(true);

    // Set credentials if provided
    match (username, password) {
        (Some(user), Some(pass)) => {
            tracing::info!("Using MQTT authentication for user: {}", user);
            mqttoptions.set_credentials(user, pass);
        }
        (Some(_), None) => {
            tracing::warn!("MQTT username provided without a password; connecting unauthenticated");
        }
        (None, Some(_)) => {
            tracing::warn!("MQTT password provided without a username; connecting unauthenticated");
        }
        (None, None) => {}
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

/// Process MQTT events and forward messages to command channel
///
/// The session is opened with clean_session=true, so the broker forgets our
/// subscription on every disconnect and rumqttc's auto-reconnect does not
/// re-send it. Each ConnAck therefore triggers a fresh subscribe; without it
/// the daemon reconnects but never receives another command.
pub async fn process_mqtt_events(
    client: AsyncClient,
    mut eventloop: EventLoop,
    topic: String,
    reconnect_delay: Duration,
    command_tx: mpsc::Sender<String>,
) {
    tracing::info!("Starting MQTT event loop");

    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let payload = String::from_utf8_lossy(&p.payload).to_string();
                tracing::debug!("Received MQTT message on topic {}: {}", p.topic, payload);

                // Forward to command handler
                if let Err(e) = command_tx.send(payload).await {
                    tracing::error!("Failed to send command to handler: {}", e);
                }
            }
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                tracing::info!("MQTT connected");
                if let Err(e) = client.subscribe(&topic, QoS::AtLeastOnce).await {
                    tracing::error!("Failed to subscribe to topic {}: {}", topic, e);
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
                tokio::time::sleep(reconnect_delay).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connect_mqtt() {
        let result = connect_mqtt("localhost", 1883, "test/topic", None, None, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_connect_mqtt_with_credentials() {
        // Test that credentials are accepted (actual authentication requires a configured broker)
        let result = connect_mqtt("localhost", 1883, "test/topic", None, Some("user"), Some("pass")).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_connect_mqtt_with_configured_client_id() {
        let result = connect_mqtt("localhost", 1883, "test/topic", Some("my-client"), None, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mqtt_event_processing() {
        let (client, eventloop) = connect_mqtt("localhost", 1883, "test/topic", None, None, None)
            .await
            .unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        // Spawn event processor
        let processor_client = client.clone();
        tokio::spawn(async move {
            process_mqtt_events(
                processor_client,
                eventloop,
                "test/topic".to_string(),
                Duration::from_secs(5),
                tx,
            ).await;
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
