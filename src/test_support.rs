// ABOUTME: Helpers shared by the library's unit tests: a local HTTP server that stops
// ABOUTME: responding, and capture of the warnings the code under test logs.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

/// A local HTTP server standing for one that stops responding. It answers its
/// first connections in turn with fixed bytes, closing each, then sends `held` on
/// the next connection and holds that one open, sending nothing more, until the
/// server is dropped.
pub(crate) struct HeldServer {
    address: std::net::SocketAddr,
    _release: std::sync::mpsc::Sender<()>,
}

impl HeldServer {
    pub(crate) fn start(answers: Vec<&'static [u8]>, held: &'static [u8]) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (release, released) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                read_request(&mut connection);
                let _ = connection.write_all(answer);
            }
            let Ok((mut connection, _)) = listener.accept() else {
                return;
            };
            read_request(&mut connection);
            let _ = connection.write_all(held);
            // Returns once the server is dropped; the connection closes with it.
            let _ = released.recv();
        });
        Self {
            address,
            _release: release,
        }
    }

    pub(crate) fn url(&self) -> String {
        format!("http://{}/held.wav", self.address)
    }
}

/// Read one request's head, up to the blank line that ends it.
fn read_request(connection: &mut std::net::TcpStream) {
    let mut request = Vec::new();
    let mut byte = [0u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        match connection.read(&mut byte) {
            Ok(1) => request.push(byte[0]),
            _ => return,
        }
    }
}

/// Collects the message of every WARN-or-worse event.
struct WarningCapture {
    warnings: Arc<Mutex<Vec<String>>>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarningCapture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() > tracing::Level::WARN {
            return;
        }
        let mut message = String::new();
        event.record(&mut MessageVisitor {
            message: &mut message,
        });
        self.warnings.lock().unwrap().push(message);
    }
}

struct MessageVisitor<'a> {
    message: &'a mut String,
}

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.message = format!("{value:?}");
        }
    }
}

/// Run `work` and return its output with the message of every WARN-or-worse event
/// logged on this thread while it ran, so a test can assert on them and they do not
/// reach the console.
pub(crate) async fn capturing_warnings<T>(
    work: impl std::future::Future<Output = T>,
) -> (T, Vec<String>) {
    use tracing_subscriber::layer::SubscriberExt;
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(WarningCapture {
        warnings: warnings.clone(),
    });
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let output = work.await;
    let captured = warnings.lock().unwrap().clone();
    (output, captured)
}
