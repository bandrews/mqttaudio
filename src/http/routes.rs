// ABOUTME: HTTP route definitions for mqttaudio REST API.
// ABOUTME: Sets up all endpoints with optional auth middleware and CORS.

use super::handlers;
use super::websocket;
use super::AppState;
use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};

/// Compare a presented token against the expected one in constant time,
/// so response timing cannot leak how much of a guess matched.
fn token_matches(candidate: &str, expected: &str) -> bool {
    let c = candidate.as_bytes();
    let e = expected.as_bytes();
    let mut diff = c.len() ^ e.len();
    for (i, &eb) in e.iter().enumerate() {
        let cb = c.get(i).copied().unwrap_or(0);
        diff |= (cb ^ eb) as usize;
    }
    diff == 0
}

/// Percent-decode a query parameter value ('+' as space), so tokens with
/// URL-encoded characters authenticate through the query form.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Authentication middleware that checks for Bearer token if configured.
async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // If no auth token is configured, allow all requests
    let Some(ref expected_token) = state.auth_token else {
        return Ok(next.run(request).await);
    };

    // Check for Authorization header
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if token_matches(token, expected_token) {
                Ok(next.run(request).await)
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        _ => {
            // Also check query parameter for simpler clients (WebSockets and
            // browsers cannot always set headers)
            let uri = request.uri();
            if let Some(query) = uri.query() {
                for param in query.split('&') {
                    if let Some(token) = param.strip_prefix("token=") {
                        if token_matches(&percent_decode(token), expected_token) {
                            return Ok(next.run(request).await);
                        }
                    }
                }
            }
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Create the main router with all endpoints.
pub fn create_router(state: AppState, cors_permissive: bool, websocket_enabled: bool) -> Router {
    // Build command routes
    let command_routes = Router::new()
        // Generic command endpoint - accepts any command JSON
        .route("/command", post(handlers::handle_command))
        // Individual command endpoints
        .route("/play", post(handlers::handle_play))
        .route("/stop", post(handlers::handle_stop))
        .route("/stopall", post(handlers::handle_stopall))
        .route("/fadeall", post(handlers::handle_fadeall))
        .route("/volume", post(handlers::handle_volume))
        .route("/seek", post(handlers::handle_seek))
        .route("/speed", post(handlers::handle_speed))
        .route("/precache", post(handlers::handle_precache))
        .route("/cache/clear", post(handlers::handle_cache_clear))
        .route("/cache/invalidate", post(handlers::handle_cache_invalidate))
        .route("/voice/stop", post(handlers::handle_voice_stop))
        .route("/voice/fade_out", post(handlers::handle_voice_fade_out))
        .route("/voice/volume", post(handlers::handle_voice_volume))
        .route("/input/volume", post(handlers::handle_input_volume))
        .route("/input/mute", post(handlers::handle_input_mute));

    // Build status routes (read-only, no auth required for basic status)
    let status_routes = Router::new()
        .route("/status", get(handlers::handle_status))
        .route("/status/samples", get(handlers::handle_samples))
        .route("/status/voices", get(handlers::handle_voices))
        .route("/status/cache", get(handlers::handle_cache_status))
        .route("/status/inputs", get(handlers::handle_inputs));

    // Health check (no auth)
    let health_route = Router::new().route("/health", get(handlers::handle_health));

    // Build the main router
    let mut app = Router::new()
        .merge(health_route)
        .merge(status_routes);

    // Add command routes with auth middleware
    let authenticated_commands = command_routes
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    app = app.merge(authenticated_commands);

    // Add WebSocket endpoint if enabled. It goes behind the same auth as the
    // command endpoints: logs leak file paths and topics. Browsers cannot set
    // an Authorization header on a WebSocket, so the query form
    // (ws://host/ws?token=...) is the way in for web clients.
    if websocket_enabled {
        let ws_route = Router::new()
            .route("/ws", get(websocket::handle_websocket))
            .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));
        app = app.merge(ws_route);
    }

    // Add CORS layer if permissive mode is enabled
    if cors_permissive {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);
        app = app.layer(cors);
    }

    // Add state
    app.with_state(state)
}
