// GigaChat – Backend entry point
//
// This file sets up the Axum web server with all routes, shared state,
// CORS policy, cookie middleware, and serves the frontend static files.

mod error;
mod handlers;
mod models;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{HeaderName, HeaderValue, Method};
use axum::{
    routing::{get, post},
    Router,
};
use supabase_rs::SupabaseClient;
use tower_cookies::CookieManagerLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;

use handlers::chat::ConversationChannels;

// ---------------------------------------------------------------------------
// Application state shared across all handlers
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub supabase: Arc<SupabaseClient>,
    pub channels: ConversationChannels,
}

// ---------------------------------------------------------------------------
// Supabase client initialisation
// ---------------------------------------------------------------------------

fn create_supabase_client() -> SupabaseClient {
    let url = std::env::var("SUPABASE_URL").expect("SUPABASE_URL must be set in .env");
    let key = std::env::var("SUPABASE_KEY").expect("SUPABASE_KEY must be set in .env");
    SupabaseClient::new(url, key).expect("Failed to create Supabase client")
}

// ---------------------------------------------------------------------------
// Build the list of allowed origins from .env
// ---------------------------------------------------------------------------

fn build_allowed_origins() -> Vec<HeaderValue> {
    let mut origins: Vec<HeaderValue> = Vec::new();

    // Read FRONTEND_URL from .env — this is the only origin we allow.
    // e.g. FRONTEND_URL=https://gigachat.vercel.app
    let frontend_url = std::env::var("FRONTEND_URL").expect("FRONTEND_URL must be set in .env");
    let url = frontend_url.trim().trim_end_matches('/').to_string();

    if url.is_empty() {
        panic!("FRONTEND_URL is set but empty — provide your Vercel URL");
    }

    match url.parse::<HeaderValue>() {
        Ok(val) => {
            println!("CORS: allowing FRONTEND_URL = {}", url);
            origins.push(val);
        }
        Err(e) => {
            panic!("FRONTEND_URL '{}' is not a valid origin: {}", url, e);
        }
    }

    origins
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Load .env file (silently ignore if missing – env vars may be set externally).
    dotenv::dotenv().ok();

    // Initialize tracing for debug logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    // Build shared state.
    let state = AppState {
        supabase: Arc::new(create_supabase_client()),
        channels: handlers::chat::new_channel_map(),
    };

    // Resolve the path to the frontend directory.
    // Default: ../frontend (relative to where `cargo run` is executed, i.e. the backend/ folder).
    let frontend_dir = std::env::var("FRONTEND_DIR").unwrap_or_else(|_| "../frontend".into());

    // Verify the frontend directory exists so the user gets a clear message.
    if !std::path::Path::new(&frontend_dir).is_dir() {
        eprintln!(
            "WARNING: Frontend directory '{}' not found. \
             Static file serving will not work. \
             Set FRONTEND_DIR env var or run from the backend/ folder.",
            frontend_dir
        );
    } else {
        println!(
            "Serving frontend from: {}",
            std::fs::canonicalize(&frontend_dir)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| frontend_dir.clone())
        );
    }

    // CORS – allow credentials (cookies) from dev origins + FRONTEND_URL from .env.
    let allowed_origins = build_allowed_origins();

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed_origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            HeaderName::from_static("content-type"),
            HeaderName::from_static("authorization"),
            HeaderName::from_static("accept"),
            HeaderName::from_static("cookie"),
        ])
        .allow_credentials(true);

    // Static file service – serves index.html, chat.html, css/, js/ etc.
    let serve_frontend = ServeDir::new(&frontend_dir);

    // Build the router with all API routes, then fall back to static files.
    let app = Router::new()
        // ── API routes ────────────────────────────────────────────────
        // Auth
        .route("/register", post(handlers::auth::register_handler))
        .route("/login", post(handlers::auth::login_handler))
        .route("/logout", post(handlers::auth::logout_handler))
        .route("/me", get(handlers::auth::me_handler))
        // Profile
        .route(
            "/profile/me",
            get(handlers::profile::get_my_profile_handler),
        )
        .route(
            "/profile/:id",
            get(handlers::profile::get_profile_handler)
                .put(handlers::profile::edit_profile_handler),
        )
        // Friends
        .route(
            "/friends",
            get(handlers::friends::get_friends_handler).post(handlers::friends::add_friend_handler),
        )
        .route(
            "/friends/pending",
            get(handlers::friends::get_pending_friends_handler),
        )
        // Conversations & messages
        .route(
            "/conversations",
            get(handlers::chat::list_conversations_handler)
                .post(handlers::chat::start_conversation_handler),
        )
        .route(
            "/conversations/:id/messages",
            get(handlers::chat::get_messages_handler),
        )
        // WebSocket
        .route("/ws/:conversation_id", get(handlers::chat::ws_handler))
        // ── Layers ────────────────────────────────────────────────────
        .layer(cors)
        .layer(CookieManagerLayer::new())
        // ── Shared state ──────────────────────────────────────────────
        .with_state(state)
        // ── Frontend fallback ─────────────────────────────────────────
        // Any request that does NOT match an API route above will be
        // served as a static file from the frontend directory.
        // e.g. GET / → frontend/index.html
        //      GET /chat.html → frontend/chat.html
        //      GET /css/variables.css → frontend/css/variables.css
        .fallback_service(serve_frontend);

    // Determine the listen address.
    let host = std::env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = std::env::var("SERVER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .expect("Invalid SERVER_HOST or SERVER_PORT");

    println!("GigaChat backend listening on http://{}", addr);
    println!("Open http://localhost:{} in your browser", port);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind TCP listener");

    axum::serve(listener, app)
        .await
        .expect("Server encountered a fatal error");
}
