// ABOUTME: Broker-gated test that process_mqtt_events subscribes on ConnAck.
// ABOUTME: This is the exact code path that recovers command delivery after a broker restart.

use mqttaudio::mqtt::client::process_mqtt_events;
use rumqttc::{AsyncClient, MqttOptions, QoS};
use std::time::Duration;
use tokio::sync::mpsc;

fn broker_tests_enabled() -> bool {
    std::env::var("MQTTAUDIO_BROKER_TESTS").is_ok()
}

/// The processor is given a client that has **not** subscribed, so the only way a
/// published message can reach `command_tx` is if `process_mqtt_events`
/// re-subscribes on `ConnAck`. With `clean_session = true`, this is precisely the
/// path that re-establishes the subscription after a broker restart — verifying it
/// in isolation (without a brittle real-restart simulation) proves the fix and
/// fails on the pre-fix code (which only logged `ConnAck`).
#[tokio::test]
async fn subscribes_on_connack_so_a_reconnect_recovers() {
    if !broker_tests_enabled() {
        return;
    }
    let topic = "mqttaudio/resubscribe/test";

    // Build the processor client WITHOUT an initial subscribe.
    let mut proc_opts = MqttOptions::new("mqttaudio-resub-proc", "localhost", 1883);
    proc_opts.set_keep_alive(Duration::from_secs(5));
    proc_opts.set_clean_session(true);
    let (proc_client, proc_eventloop) = AsyncClient::new(proc_opts, 10);

    let (tx, mut rx) = mpsc::channel::<String>(50);
    let pc = proc_client.clone();
    let topic_owned = topic.to_string();
    tokio::spawn(async move {
        process_mqtt_events(pc, topic_owned, proc_eventloop, tx).await;
    });

    // Let the processor connect and subscribe on its first ConnAck.
    tokio::time::sleep(Duration::from_millis(800)).await;

    // Publish from a separate client.
    let mut pub_opts = MqttOptions::new("mqttaudio-resub-pub", "localhost", 1883);
    pub_opts.set_keep_alive(Duration::from_secs(5));
    let (pubber, mut pub_loop) = AsyncClient::new(pub_opts, 10);
    tokio::spawn(async move { while pub_loop.poll().await.is_ok() {} });
    tokio::time::sleep(Duration::from_millis(300)).await;
    pubber
        .publish(topic, QoS::AtLeastOnce, false, "hello")
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out: process_mqtt_events did not subscribe on ConnAck")
        .expect("command channel closed");
    assert_eq!(msg, "hello");
}
