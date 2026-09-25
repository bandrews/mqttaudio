// ABOUTME: Asks a running daemon whether it is ready, for container and service health checks.
// ABOUTME: Finds the HTTP server's address in the effective configuration and requests /ready.

use crate::config::HttpConfig;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Where to ask a daemon for its readiness, or why it cannot be asked.
#[derive(Debug, PartialEq, Eq)]
pub enum Probe {
    /// The HTTP server is off, so there is no readiness to check.
    HttpDisabled,
    /// The HTTP server takes a random free port, so it cannot be found.
    RandomPort,
    /// The daemon's `/ready` URL.
    Url(String),
}

/// Locate the `/ready` route of a daemon running with this HTTP configuration. A
/// server bound to every address is reached on loopback.
pub fn probe(http: &HttpConfig) -> Probe {
    if !http.enabled {
        return Probe::HttpDisabled;
    }
    if http.port == 0 {
        return Probe::RandomPort;
    }
    let host = match http.bind_address.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        Ok(IpAddr::V6(ip)) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        Ok(ip) => ip,
        Err(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
    };
    Probe::Url(format!("http://{}/ready", SocketAddr::new(host, http.port)))
}

/// Request `url` and succeed only on a `2xx` answer. The request bypasses any
/// configured proxy, since the daemon is local, and gives up after 3 seconds.
pub async fn check(url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| format!("cannot create an HTTP client: {e}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("{url} did not answer: {e}"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!("{url} answered {status}: {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HttpConfig;

    fn http(enabled: bool, bind_address: &str, port: u16) -> HttpConfig {
        HttpConfig {
            enabled,
            port,
            bind_address: bind_address.to_string(),
            ..HttpConfig::default()
        }
    }

    #[test]
    fn a_disabled_server_has_nothing_to_check() {
        assert_eq!(probe(&http(false, "127.0.0.1", 8080)), Probe::HttpDisabled);
    }

    #[test]
    fn a_random_port_cannot_be_found() {
        assert_eq!(probe(&http(true, "127.0.0.1", 0)), Probe::RandomPort);
    }

    #[test]
    fn a_wildcard_bind_is_checked_on_loopback() {
        assert_eq!(
            probe(&http(true, "0.0.0.0", 8080)),
            Probe::Url("http://127.0.0.1:8080/ready".to_string())
        );
        assert_eq!(
            probe(&http(true, "::", 8080)),
            Probe::Url("http://[::1]:8080/ready".to_string())
        );
    }

    #[test]
    fn a_specific_bind_is_checked_at_that_address() {
        assert_eq!(
            probe(&http(true, "10.0.0.5", 9000)),
            Probe::Url("http://10.0.0.5:9000/ready".to_string())
        );
    }

    /// Answer one request on a local port with `status` and `body`, returning the
    /// readiness URL.
    async fn serve_once(status: &'static str, body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/ready", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });
        url
    }

    #[tokio::test]
    async fn a_ready_daemon_passes() {
        let url = serve_once("200 OK", r#"{"status":"ready"}"#).await;
        assert!(check(&url).await.is_ok());
    }

    #[tokio::test]
    async fn a_daemon_that_is_not_ready_fails_with_its_answer() {
        let url = serve_once("503 Service Unavailable", r#"{"status":"not_ready"}"#).await;
        let error = check(&url).await.unwrap_err();
        assert!(error.contains("503"), "{error}");
        assert!(error.contains("not_ready"), "{error}");
    }

    #[tokio::test]
    async fn an_unreachable_daemon_fails() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/ready", listener.local_addr().unwrap());
        drop(listener);
        assert!(check(&url).await.is_err());
    }
}
