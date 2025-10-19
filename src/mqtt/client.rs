// ABOUTME: MQTT connection management using rumqttc.
// ABOUTME: Handles connection, reconnection, and message receiving.

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum MqttError {
    #[allow(dead_code)] // May be used for explicit connection errors in future
    ConnectionError(rumqttc::ClientError),
    SubscriptionError(rumqttc::ClientError),
}

impl std::fmt::Display for MqttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MqttError::ConnectionError(e) => write!(f, "MQTT connection error: {}", e),
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
) -> Result<(AsyncClient, EventLoop), MqttError> {
    tracing::info!("Connecting to MQTT broker: {}:{}", server, port);

    // Use unique client ID with random suffix to avoid conflicts
    use rand::Rng;
    let random_suffix: u32 = rand::thread_rng().gen();
    let client_id = format!("mqttaudio_{:08x}", random_suffix);

    let mut mqttoptions = MqttOptions::new(client_id, server, port);
    mqttoptions.set_keep_alive(Duration::from_secs(60));
    mqttoptions.set_clean_session(true);

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
pub async fn process_mqtt_events(
    mut eventloop: EventLoop,
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

    // Note: These tests require a running MQTT broker
    // They are marked as ignored to avoid breaking CI

    #[tokio::test]
    #[ignore]
    async fn test_connect_mqtt() {
        let result = connect_mqtt("localhost", 1883, "test/topic").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore]
    async fn test_mqtt_event_processing() {
        let (client, eventloop) = connect_mqtt("localhost", 1883, "test/topic")
            .await
            .unwrap();

        let (tx, mut rx) = mpsc::channel(10);

        // Spawn event processor
        tokio::spawn(async move {
            process_mqtt_events(eventloop, tx).await;
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
