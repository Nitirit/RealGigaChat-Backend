use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;
use tower_cookies::{Cookie, Cookies};
use uuid::Uuid;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

use crate::error::ApiError;
use crate::models::{AuthResponse, LoginRequest, ProfileRow, RegisterRequest};
use crate::AppState;

/// Name of the session cookie.
const SESSION_COOKIE: &str = "gigachat_session";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write the user-id into a cookie so subsequent requests are authenticated.
pub fn set_session(cookies: &Cookies, user_id: Uuid) {
    let mut cookie = Cookie::new(SESSION_COOKIE, user_id.to_string());
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookies.add(cookie);
}

/// Read and parse the user-id from the session cookie.
pub fn get_session(cookies: &Cookies) -> Result<Uuid, ApiError> {
    let cookie = cookies.get(SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
    let value = cookie.value().to_string();
    Uuid::parse_str(&value).map_err(|_| ApiError::Unauthorized)
}

/// Hash a plaintext password with Argon2.
fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| ApiError::Internal(format!("Hashing failed: {}", e)))?;
    Ok(hash.to_string())
}

/// Verify a plaintext password against an Argon2 hash string.
fn verify_password(password: &str, hash: &str) -> Result<bool, ApiError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| ApiError::Internal(format!("Bad stored hash: {}", e)))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Direct insert into Supabase via reqwest so we can read the full error body.
/// supabase_rs's insert() only gives "400 Bad Request" with no details.
async fn supabase_insert(table: &str, body: serde_json::Value) -> Result<String, ApiError> {
    let supabase_url = std::env::var("SUPABASE_URL")
        .map_err(|_| ApiError::Internal("SUPABASE_URL not set".into()))?;
    let supabase_key = std::env::var("SUPABASE_KEY")
        .map_err(|_| ApiError::Internal("SUPABASE_KEY not set".into()))?;

    let url = format!("{}/rest/v1/{}", supabase_url.trim_end_matches('/'), table);

    eprintln!("[supabase_insert] POST {} body={}", url, body);

    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .header("apikey", &supabase_key)
        .header("Authorization", format!("Bearer {}", supabase_key))
        .header("Content-Type", "application/json")
        // Ask Supabase to return the inserted row so we can read the generated id
        .header("Prefer", "return=representation")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            eprintln!("[supabase_insert] Network error: {}", e);
            ApiError::Database(format!("Network error talking to Supabase: {}", e))
        })?;

    let status = res.status();
    let response_text = res
        .text()
        .await
        .unwrap_or_else(|_| "(could not read body)".into());

    eprintln!(
        "[supabase_insert] Response status={} body={}",
        status, response_text
    );

    if !status.is_success() {
        // Parse the error body for a friendlier message
        let detail = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response_text) {
            json.get("message")
                .or_else(|| json.get("msg"))
                .or_else(|| json.get("details"))
                .or_else(|| json.get("hint"))
                .and_then(|v| v.as_str())
                .unwrap_or(&response_text)
                .to_string()
        } else {
            response_text.clone()
        };

        if status.as_u16() == 404 {
            return Err(ApiError::Database(format!(
                "Table '{}' not found. Did you run the SQL from SCHEMA.md in your Supabase SQL Editor? ({})",
                table, detail
            )));
        } else if status.as_u16() == 403 {
            return Err(ApiError::Database(format!(
                "Permission denied on '{}'. Disable Row Level Security (RLS) or use the service-role key. ({})",
                table, detail
            )));
        } else if status.as_u16() == 409 {
            return Err(ApiError::BadRequest(format!("Duplicate entry: {}", detail)));
        } else {
            return Err(ApiError::Database(format!(
                "Supabase error {} on '{}': {}",
                status.as_u16(),
                table,
                detail
            )));
        }
    }

    // Parse the response to extract the id of the inserted row.
    // Supabase returns an array like [{ "id": "...", ... }]
    let rows: Vec<serde_json::Value> = serde_json::from_str(&response_text).map_err(|e| {
        eprintln!(
            "[supabase_insert] Failed to parse response as JSON array: {} body={}",
            e, response_text
        );
        ApiError::Internal(format!("Unexpected Supabase response: {}", response_text))
    })?;

    if rows.is_empty() {
        return Err(ApiError::Internal(
            "Supabase returned empty array after insert".into(),
        ));
    }

    let id = rows[0].get("id").and_then(|v| v.as_str()).ok_or_else(|| {
        ApiError::Internal(format!(
            "Supabase response missing 'id' field: {}",
            response_text
        ))
    })?;

    Ok(id.to_string())
}

// ---------------------------------------------------------------------------
// POST /register
// ---------------------------------------------------------------------------

pub async fn register_handler(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(body): Json<RegisterRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // --- validate input ---
    let username = body.username.trim().to_string();
    let password = body.password.clone();

    if username.is_empty() {
        return Err(ApiError::BadRequest("Username cannot be empty".into()));
    }
    if password.len() < 6 {
        return Err(ApiError::BadRequest(
            "Password must be at least 6 characters".into(),
        ));
    }

    // --- check if username already taken ---
    let rows = state
        .supabase
        .select("profiles")
        .eq("username", &username)
        .execute()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            eprintln!("[register] Failed to query profiles table: {}", msg);
            ApiError::Database(format!("Failed to check username: {}", msg))
        })?;

    if !rows.is_empty() {
        return Err(ApiError::BadRequest("Username is already taken".into()));
    }

    // --- hash the password ---
    let password_hash = hash_password(&password)?;

    // --- generate a UUID for the new user ---
    // The profiles.id column has no DEFAULT in this database, so we must provide it.
    let user_id = Uuid::new_v4();

    let display_name = body.display_name.unwrap_or_else(|| username.clone());

    // --- insert into Supabase using direct HTTP ---
    // We bypass supabase_rs for insert because its error messages are opaque
    // ("400 Bad Request" with no details). Direct reqwest lets us read
    // the full Supabase error body for debugging.
    let insert_body = json!({
        "id": user_id.to_string(),
        "username": username,
        "password_hash": password_hash,
        "display_name": display_name,
    });

    eprintln!(
        "[register] Inserting new profile for username={} id={}",
        username, user_id
    );

    let _returned_id = supabase_insert("profiles", insert_body).await?;

    eprintln!("[register] Insert succeeded for user_id={}", user_id);

    // --- set session cookie so the user is logged in immediately ---
    set_session(&cookies, user_id);

    eprintln!(
        "[register] Success! user_id={} username={}",
        user_id, username
    );

    Ok(Json(AuthResponse { user_id, username }))
}

// ---------------------------------------------------------------------------
// POST /login
// ---------------------------------------------------------------------------

pub async fn login_handler(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(body): Json<LoginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let username = body.username.trim().to_string();
    let password = body.password.clone();

    if username.is_empty() || password.is_empty() {
        return Err(ApiError::BadRequest(
            "Username and password are required".into(),
        ));
    }

    // --- fetch user by username ---
    let rows = state
        .supabase
        .select("profiles")
        .eq("username", &username)
        .execute()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            eprintln!("[login] Failed to query profiles table: {}", msg);
            ApiError::Database(format!("Failed to look up user: {}", msg))
        })?;

    if rows.is_empty() {
        return Err(ApiError::InvalidCredentials);
    }

    let profile: ProfileRow = serde_json::from_value(rows[0].clone()).map_err(|e| {
        eprintln!("[login] Failed to parse profile row: {}", e);
        eprintln!("[login] Raw row data: {}", rows[0]);
        ApiError::Database(format!("Failed to parse profile data: {}", e))
    })?;

    // --- verify password ---
    let stored_hash = profile
        .password_hash
        .as_deref()
        .ok_or(ApiError::Internal("No password hash stored".into()))?;

    if !verify_password(&password, stored_hash)? {
        return Err(ApiError::InvalidCredentials);
    }

    // --- set session ---
    set_session(&cookies, profile.id);

    eprintln!(
        "[login] Success! user_id={} username={}",
        profile.id, profile.username
    );

    Ok(Json(AuthResponse {
        user_id: profile.id,
        username: profile.username,
    }))
}

// ---------------------------------------------------------------------------
// POST /logout
// ---------------------------------------------------------------------------

pub async fn logout_handler(cookies: Cookies) -> impl IntoResponse {
    // Remove the session cookie by setting it to empty with max-age 0.
    let mut cookie = Cookie::new(SESSION_COOKIE, "");
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_max_age(Some(tower_cookies::cookie::time::Duration::ZERO));
    cookies.add(cookie);

    Json(json!({ "status": "logged out" }))
}

// ---------------------------------------------------------------------------
// GET /me
// ---------------------------------------------------------------------------

pub async fn me_handler(cookies: Cookies) -> Result<impl IntoResponse, ApiError> {
    let user_id = get_session(&cookies)?;
    Ok(Json(json!({ "user_id": user_id })))
}
