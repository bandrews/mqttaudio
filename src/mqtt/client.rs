// ABOUTME: MQTT connection management using rumqttc.
// ABOUTME: Handles connection, reconnection, and message receiving.

use crate::config::{MqttConfig, MqttTlsConfig};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, Publish, QoS, Transport};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum MqttError {
    SubscriptionError(rumqttc::ClientError),
    TlsConfig(String),
}

impl std::fmt::Display for MqttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MqttError::SubscriptionError(e) => write!(f, "MQTT subscription error: {}", e),
            MqttError::TlsConfig(m) => write!(f, "MQTT TLS configuration error: {}", m),
        }
    }
}

impl std::error::Error for MqttError {}

/// Which transport `connect_mqtt` will use for a config.
#[derive(Debug, PartialEq, Eq)]
pub enum TransportKind {
    Tcp,
    Tls,
}

/// Choose the MQTT transport. TLS is strictly opt-in via `[mqtt.tls]`; the port is
/// deliberately ignored, so a legacy plaintext broker on 8883 keeps working.
pub fn select_transport_kind(cfg: &MqttConfig) -> TransportKind {
    if cfg.tls.is_some() {
        TransportKind::Tls
    } else {
        TransportKind::Tcp
    }
}

/// A warning message when credentials would cross a plaintext link to a
/// non-loopback broker. Loopback brokers and TLS connections are exempt.
pub fn cleartext_credentials_warning(cfg: &MqttConfig) -> Option<String> {
    let has_credentials = cfg.username.is_some() || cfg.password.is_some();
    if !has_credentials || cfg.tls.is_some() || is_loopback_host(&cfg.server) {
        return None;
    }
    Some(format!(
        "MQTT credentials are being sent in cleartext to non-loopback broker '{}'. \
         Configure [mqtt.tls] to encrypt the connection.",
        cfg.server
    ))
}

/// The literal `localhost`, or any IP literal that is a loopback address.
fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Build a rumqttc TLS transport: trust a configured CA certificate, or fall
/// back to the system root certificate store.
fn build_tls_transport(tls: &MqttTlsConfig) -> Result<Transport, MqttError> {
    match &tls.ca_path {
        Some(path) => {
            let ca = std::fs::read(path)
                .map_err(|e| MqttError::TlsConfig(format!("reading CA '{}': {}", path, e)))?;
            Ok(Transport::tls(ca, None, None))
        }
        None => Ok(Transport::tls_with_default_config()),
    }
}

/// Connect to MQTT broker and subscribe to topic
pub async fn connect_mqtt(
    cfg: &MqttConfig,
    topic: &str,
) -> Result<(AsyncClient, EventLoop), MqttError> {
    tracing::info!("Connecting to MQTT broker: {}:{}", cfg.server, cfg.port);

    // Use unique client ID with random suffix to avoid conflicts
    use rand::Rng;
    let random_suffix: u32 = rand::thread_rng().gen();
    let client_id = format!("mqttaudio_{:08x}", random_suffix);

    let mut mqttoptions = MqttOptions::new(client_id, &cfg.server, cfg.port);
    mqttoptions.set_keep_alive(Duration::from_secs(60));
    mqttoptions.set_clean_session(true);

    if let Some(warning) = cleartext_credentials_warning(cfg) {
        tracing::warn!("{}", warning);
    }

    match select_transport_kind(cfg) {
        TransportKind::Tcp => {}
        TransportKind::Tls => {
            mqttoptions.set_transport(build_tls_transport(cfg.tls.as_ref().unwrap())?);
            tracing::info!("MQTT TLS enabled");
        }
    }

    // Set credentials if provided
    if let (Some(user), Some(pass)) = (cfg.username.as_deref(), cfg.password.as_deref()) {
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

    fn config(server: &str, port: u16) -> MqttConfig {
        MqttConfig {
            server: server.to_string(),
            port,
            ..MqttConfig::default()
        }
    }

    #[test]
    fn transport_is_tls_only_when_configured() {
        let mut cfg = config("localhost", 8883);
        // No [mqtt.tls] → plain TCP even on the conventional TLS port.
        assert_eq!(select_transport_kind(&cfg), TransportKind::Tcp);
        cfg.tls = Some(MqttTlsConfig::default());
        assert_eq!(select_transport_kind(&cfg), TransportKind::Tls);
    }

    #[test]
    fn cleartext_warning_fires_only_for_remote_plaintext_credentials() {
        let mut cfg = config("broker.example.com", 1883);
        cfg.username = Some("u".to_string());
        cfg.password = Some("p".to_string());
        // Remote broker + credentials + no TLS → warn.
        assert!(cleartext_credentials_warning(&cfg).is_some());

        // Loopback brokers are exempt.
        for host in ["localhost", "127.0.0.1", "::1"] {
            cfg.server = host.to_string();
            assert!(
                cleartext_credentials_warning(&cfg).is_none(),
                "loopback host {host} should not warn"
            );
        }

        // TLS is exempt.
        cfg.server = "broker.example.com".to_string();
        cfg.tls = Some(MqttTlsConfig::default());
        assert!(cleartext_credentials_warning(&cfg).is_none());

        // No credentials → no warning.
        cfg.tls = None;
        cfg.username = None;
        cfg.password = None;
        assert!(cleartext_credentials_warning(&cfg).is_none());
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
        let result = connect_mqtt(&config("localhost", 1883), "test/topic").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_connect_mqtt_with_credentials() {
        if !broker_tests_enabled() {
            return;
        }
        // Test that credentials are accepted (actual authentication requires a configured broker)
        let mut cfg = config("localhost", 1883);
        cfg.username = Some("user".to_string());
        cfg.password = Some("pass".to_string());
        let result = connect_mqtt(&cfg, "test/topic").await;
        assert!(result.is_ok());
    }

    /// TLS connect against a TLS-enabled mosquitto. Runs only when the broker
    /// tests are enabled AND `MQTTAUDIO_TLS_CA` points at the broker's CA (the
    /// Lane A Docker pipeline sets both). Polls the event loop until `ConnAck`,
    /// which only arrives after the TLS handshake actually completes.
    #[tokio::test]
    async fn test_connect_mqtt_tls() {
        if !broker_tests_enabled() {
            return;
        }
        let ca_path = match std::env::var("MQTTAUDIO_TLS_CA") {
            Ok(p) => p,
            Err(_) => return,
        };
        let mut cfg = config("localhost", 8883);
        cfg.tls = Some(MqttTlsConfig {
            ca_path: Some(ca_path),
        });
        let (_client, mut eventloop) = connect_mqtt(&cfg, "test/topic")
            .await
            .expect("connect_mqtt should build a TLS client");

        let connected = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => return true,
                    Ok(_) => continue,
                    Err(e) => panic!("TLS connect error: {e}"),
                }
            }
        })
        .await
        .expect("timed out waiting for ConnAck over TLS");
        assert!(connected);
    }

    #[tokio::test]
    async fn test_mqtt_event_processing() {
        if !broker_tests_enabled() {
            return;
        }
        let (client, eventloop) = connect_mqtt(&config("localhost", 1883), "test/topic")
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
