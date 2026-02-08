use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use serde_json::json;
use tower_cookies::Cookies;
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::auth::get_session;
use crate::models::{EditProfileRequest, ProfileResponse, ProfileRow};
use crate::AppState;

// ---------------------------------------------------------------------------
// GET /profile/{id}
// ---------------------------------------------------------------------------

pub async fn get_profile_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let profile = fetch_profile_by_id(&state, id).await?;
    let response: ProfileResponse = profile.into();
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// GET /profile/me  –  shortcut that uses the session cookie
// ---------------------------------------------------------------------------

pub async fn get_my_profile_handler(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = get_session(&cookies)?;
    let profile = fetch_profile_by_id(&state, user_id).await?;
    let response: ProfileResponse = profile.into();
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// PUT /profile/{id}
// ---------------------------------------------------------------------------

pub async fn edit_profile_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    cookies: Cookies,
    Json(body): Json<EditProfileRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Only the owner can edit their own profile.
    let session_user = get_session(&cookies)?;
    if session_user != id {
        return Err(ApiError::Unauthorized);
    }

    // Build the update payload with only the fields the client provided.
    let mut update = json!({});
    if let Some(ref display_name) = body.display_name {
        update["display_name"] = json!(display_name);
    }
    if let Some(ref avatar_url) = body.avatar_url {
        update["avatar_url"] = json!(avatar_url);
    }
    if let Some(ref bio) = body.bio {
        update["bio"] = json!(bio);
    }

    // If nothing was provided there is nothing to do.
    if update.as_object().map_or(true, |m| m.is_empty()) {
        return Err(ApiError::BadRequest(
            "Provide at least one field to update".into(),
        ));
    }

    state
        .supabase
        .update("profiles", &id.to_string(), update)
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    // Return the updated profile so the client can refresh its state.
    let updated = fetch_profile_by_id(&state, id).await?;
    let response: ProfileResponse = updated.into();
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// Internal helper
// ---------------------------------------------------------------------------

/// Fetch a single profile row from Supabase by its UUID.
async fn fetch_profile_by_id(state: &AppState, id: Uuid) -> Result<ProfileRow, ApiError> {
    let rows = state
        .supabase
        .select("profiles")
        .eq("id", &id.to_string())
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    if rows.is_empty() {
        return Err(ApiError::NotFound("Profile not found".into()));
    }

    let profile: ProfileRow =
        serde_json::from_value(rows[0].clone()).map_err(|e| ApiError::Database(e.to_string()))?;

    Ok(profile)
}
