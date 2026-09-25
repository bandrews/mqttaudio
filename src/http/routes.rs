// ABOUTME: HTTP route definitions for mqttaudio REST API.
// ABOUTME: Sets up all endpoints with optional auth, the cross-site check and CORS.

use super::handlers;
use super::websocket;
use super::AppState;
use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use tower_http::cors::{AllowHeaders, Any, CorsLayer};

/// Constant-time string equality (avoids leaking the token via compare timing).
/// The length is allowed to differ-fast; token length is not the secret.
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
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
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
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

/// Authentication middleware. With no token configured the request passes (open
/// mode) unless `require_auth` is set, in which case it fails closed. A token is
/// accepted via `Authorization: Bearer <token>` or the `?token=` query param,
/// compared in constant time.
async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(ref expected_token) = state.auth_token else {
        if state.require_auth {
            // require_auth with no token configured -> nothing can authenticate.
            return Err(StatusCode::UNAUTHORIZED);
        }
        return Ok(next.run(request).await);
    };

    // Authorization: Bearer <token>
    let bearer_ok = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
        .map(|token| ct_eq(token, expected_token))
        .unwrap_or(false);
    if bearer_ok {
        return Ok(next.run(request).await);
    }

    // ?token= convenience parameter for simpler clients.
    if let Some(query) = request.uri().query() {
        for param in query.split('&') {
            if let Some(token) = param.strip_prefix("token=") {
                if ct_eq(&percent_decode(token), expected_token) {
                    return Ok(next.run(request).await);
                }
            }
        }
    }

    Err(StatusCode::UNAUTHORIZED)
}

/// Refuse a request a browser sends from a page on another site (`Sec-Fetch-Site:
/// cross-site`). Without CORS such a page cannot read the answers or send JSON,
/// but it can still send the body-less commands and open the WebSockets. Clients
/// that are not browsers send no such header and are served.
async fn refuse_cross_site(request: Request<Body>, next: Next) -> Response {
    let cross_site = request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|site| site.as_bytes().eq_ignore_ascii_case(b"cross-site"));
    if cross_site {
        tracing::debug!(
            "Refused a request for {} from a page on another site",
            request.uri().path()
        );
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "success": false,
                "error": "Requests from pages on other sites are refused; set \
                          http.cors_permissive to allow them",
            })),
        )
            .into_response();
    }
    next.run(request).await
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
        .route("/cache/reload", post(handlers::handle_cache_reload))
        .route("/voice/stop", post(handlers::handle_voice_stop))
        .route("/voice/fade_out", post(handlers::handle_voice_fade_out))
        .route("/voice/volume", post(handlers::handle_voice_volume))
        .route("/input/volume", post(handlers::handle_input_volume))
        .route("/input/mute", post(handlers::handle_input_mute))
        .route("/talkback/acquire", post(handlers::handle_talkback_acquire))
        .route("/talkback/release", post(handlers::handle_talkback_release))
        .route(
            "/talkback/hard-mute",
            post(handlers::handle_talkback_hard_mute),
        )
        // Turning telemetry on changes daemon state and adds real-time work, so
        // it is guarded like a command; GET /telemetry stays with the status routes.
        .route("/telemetry", post(handlers::handle_telemetry_set));

    // Build status routes (read-only, no auth required for basic status). These
    // include /version and /metrics, which are gated alongside the status routes
    // when require_auth is set and open otherwise.
    let status_routes = Router::new()
        .route("/status", get(handlers::handle_status))
        .route("/status/samples", get(handlers::handle_samples))
        .route("/status/voices", get(handlers::handle_voices))
        .route("/status/cache", get(handlers::handle_cache_status))
        .route("/status/inputs", get(handlers::handle_inputs))
        .route("/status/talkback", get(handlers::handle_talkback_status))
        .route("/version", get(handlers::handle_version))
        .route("/metrics", get(handlers::handle_metrics))
        // Per-output-channel peak meters poll fallback (Sprint W7).
        .route("/status/meters", get(handlers::handle_meters))
        // Read-only running config, secrets redacted (Sprint W8, DW11).
        .route("/config", get(handlers::handle_config))
        // Whether live-position and meter telemetry is on (Sprint W6, DW3).
        .route("/telemetry", get(handlers::handle_telemetry_get));

    // Health check (no auth)
    let health_route = Router::new()
        .route("/health", get(handlers::handle_health))
        .route("/ready", get(handlers::handle_ready));

    // Command routes always carry the auth middleware (enforced when a token is
    // set, or always when require_auth locks the whole API).
    let authenticated_commands = command_routes.layer(middleware::from_fn_with_state(
        state.clone(),
        auth_middleware,
    ));

    // /health and /ready are always open (liveness and readiness probes). Status
    // routes are open unless require_auth is set; the WebSockets carry the auth
    // middleware either way, so they need the token whenever one is set.
    let mut app = Router::new();

    if state.require_auth {
        let protected_status = status_routes.layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));
        app = app.merge(protected_status).merge(authenticated_commands);
        if websocket_enabled {
            let ws = Router::new()
                .route("/ws", get(websocket::handle_websocket))
                .route("/ws/state", get(websocket::handle_state_websocket))
                .layer(middleware::from_fn_with_state(
                    state.clone(),
                    auth_middleware,
                ));
            app = app.merge(ws);
        }
    } else {
        app = app.merge(status_routes).merge(authenticated_commands);
        if websocket_enabled {
            app = app.merge(
                Router::new()
                    .route("/ws", get(websocket::handle_websocket))
                    .route("/ws/state", get(websocket::handle_state_websocket))
                    .layer(middleware::from_fn_with_state(
                        state.clone(),
                        auth_middleware,
                    )),
            );
        }
    }

    // Pages on other sites are served only with CORS on, which then answers them
    // for any origin. The allowed headers mirror the preflight's request, since
    // browsers do not count `Authorization` as covered by a `*`.
    if cors_permissive {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(AllowHeaders::mirror_request());
        app = app.merge(health_route).layer(cors);
    } else {
        app = app
            .layer(middleware::from_fn(refuse_cross_site))
            .merge(health_route);
    }

    app.with_state(state)
}
