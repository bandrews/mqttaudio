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
                if ct_eq(token, expected_token) {
                    return Ok(next.run(request).await);
                }
            }
        }
    }

    Err(StatusCode::UNAUTHORIZED)
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
        .route("/input/mute", post(handlers::handle_input_mute));

    // Build status routes (read-only, no auth required for basic status). These
    // include /version and /metrics, which are gated alongside the status routes
    // when require_auth is set and open otherwise.
    let status_routes = Router::new()
        .route("/status", get(handlers::handle_status))
        .route("/status/samples", get(handlers::handle_samples))
        .route("/status/voices", get(handlers::handle_voices))
        .route("/status/cache", get(handlers::handle_cache_status))
        .route("/status/inputs", get(handlers::handle_inputs))
        .route("/version", get(handlers::handle_version))
        .route("/metrics", get(handlers::handle_metrics));

    // Health check (no auth)
    let health_route = Router::new().route("/health", get(handlers::handle_health));

    // Command routes always carry the auth middleware (enforced when a token is
    // set, or always when require_auth locks the whole API).
    let authenticated_commands = command_routes.layer(middleware::from_fn_with_state(
        state.clone(),
        auth_middleware,
    ));

    // /health is always open (liveness probe). Status and ws are open by default
    // but gated when require_auth is set.
    let mut app = Router::new().merge(health_route);

    if state.require_auth {
        let protected_status = status_routes.layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));
        app = app.merge(protected_status).merge(authenticated_commands);
        if websocket_enabled {
            let ws = Router::new()
                .route("/ws", get(websocket::handle_websocket))
                .layer(middleware::from_fn_with_state(
                    state.clone(),
                    auth_middleware,
                ));
            app = app.merge(ws);
        }
    } else {
        app = app.merge(status_routes).merge(authenticated_commands);
        if websocket_enabled {
            app = app.route("/ws", get(websocket::handle_websocket));
        }
    }

    // Add CORS layer if permissive mode is enabled
    if cors_permissive {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);
        app = app.layer(cors);
    }

    app.with_state(state)
}
