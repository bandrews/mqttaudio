// ABOUTME: Integration test proving MQTT commands still arrive after the broker restarts.
// ABOUTME: Spawns its own mosquitto on a random port; requires mosquitto to be installed.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use mqttaudio::mqtt::client::{connect_mqtt, process_mqtt_events};
use mqttaudio::mqtt::commands::CommandRequest;
use rumqttc::{AsyncClient, MqttOptions, QoS};
use tokio::sync::mpsc;

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn start_broker(port: u16) -> Child {
    let child = Command::new("mosquitto")
        .args(["-p", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("mosquitto must be installed for this test (apt-get install mosquitto)");
    // Give the broker a moment to start listening
    std::thread::sleep(Duration::from_millis(300));
    child
}

async fn publish_until_received(
    port: u16,
    topic: &str,
    payload: &str,
    rx: &mut mpsc::Receiver<CommandRequest>,
) -> Option<String> {
    // Publish repeatedly so the test does not race the subscriber's
    // (re)subscription, and poll the receiving channel between attempts.
    for _ in 0..40 {
        let mut opts = MqttOptions::new(format!("pub_{}", rand_suffix()), "127.0.0.1", port);
        opts.set_keep_alive(Duration::from_secs(5));
        let (publisher, mut pub_loop) = AsyncClient::new(opts, 10);
        let pub_driver = tokio::spawn(async move {
            for _ in 0..20 {
                if pub_loop.poll().await.is_err() {
                    break;
                }
            }
        });
        let _ = publisher
            .publish(topic, QoS::AtLeastOnce, false, payload)
            .await;

        if let Ok(Some(received)) =
            tokio::time::timeout(Duration::from_millis(500), rx.recv()).await
        {
            pub_driver.abort();
            return Some(received.payload);
        }
        pub_driver.abort();
    }
    None
}

fn rand_suffix() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
}

#[tokio::test]
async fn test_commands_still_arrive_after_broker_restart() {
    let port = free_port();
    let topic = "reconnect/test";

    let mut broker = start_broker(port);

    let (client, eventloop) = connect_mqtt(
        &mqttaudio::config::MqttConfig {
            server: "127.0.0.1".into(),
            port,
            ..Default::default()
        },
        topic,
    )
    .await
    .expect("initial connect should succeed");

    let (tx, mut rx) = mpsc::channel(10);
    let processor_client = client.clone();
    let processor = tokio::spawn(async move {
        process_mqtt_events(
            processor_client,
            topic.to_string(),
            eventloop,
            tx,
            Duration::from_secs(1),
        )
        .await;
    });

    // Sanity: commands arrive on the first connection
    let first = publish_until_received(port, topic, r#"{"command":"stopall"}"#, &mut rx).await;
    assert!(first.is_some(), "Command should arrive before the restart");

    // Restart the broker. With clean_session=true the broker forgets our
    // subscription, so only an explicit resubscribe on reconnect keeps
    // commands flowing.
    broker.kill().unwrap();
    broker.wait().unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut broker = start_broker(port);

    let second = publish_until_received(port, topic, r#"{"command":"stopall"}"#, &mut rx).await;

    processor.abort();
    broker.kill().unwrap();
    broker.wait().unwrap();

    assert!(
        second.is_some(),
        "Command should arrive after the broker restarts - the client must resubscribe on reconnect"
    );
}
