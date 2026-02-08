use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;
use tower_cookies::Cookies;
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::auth::get_session;
use crate::models::{AddFriendRequest, FriendInfo, FriendRow, ProfileRow};
use crate::AppState;

// ---------------------------------------------------------------------------
// POST /friends
// ---------------------------------------------------------------------------

pub async fn add_friend_handler(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(body): Json<AddFriendRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let me = get_session(&cookies)?;

    if me == body.friend_id {
        return Err(ApiError::BadRequest(
            "You cannot add yourself as a friend".into(),
        ));
    }

    // Check that the friend actually exists.
    let friend_rows = state
        .supabase
        .select("profiles")
        .eq("id", &body.friend_id.to_string())
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    if friend_rows.is_empty() {
        return Err(ApiError::NotFound("User not found".into()));
    }

    // Enforce user_a < user_b so the UNIQUE constraint works.
    let (user_a, user_b) = if me < body.friend_id {
        (me, body.friend_id)
    } else {
        (body.friend_id, me)
    };

    // Check if a friendship row already exists between these two users.
    let existing_rows = state
        .supabase
        .select("friends")
        .eq("user_a", &user_a.to_string())
        .eq("user_b", &user_b.to_string())
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    if !existing_rows.is_empty() {
        // A row already exists – check its status.
        let row: FriendRow = serde_json::from_value(existing_rows[0].clone())
            .map_err(|e| ApiError::Database(e.to_string()))?;

        match row.status.as_str() {
            "accepted" => {
                return Err(ApiError::BadRequest("You are already friends".into()));
            }
            "pending" => {
                // If *I* am the one who received the request, accept it.
                // If I sent it, tell the user it's already pending.
                // In the simple model we just accept it from either side.
                let row_id = row.id.map(|i| i.to_string()).unwrap_or_default();
                state
                    .supabase
                    .update("friends", &row_id, json!({ "status": "accepted" }))
                    .await
                    .map_err(|e| ApiError::Database(e.to_string()))?;

                return Ok(Json(json!({ "status": "accepted" })));
            }
            "blocked" => {
                return Err(ApiError::BadRequest("This friendship is blocked".into()));
            }
            other => {
                return Err(ApiError::Internal(format!(
                    "Unknown friend status: {}",
                    other
                )));
            }
        }
    }

    // No existing row – create a new friendship with status "accepted" immediately.
    // No need to wait for the other user to accept.
    let insert_body = json!({
        "user_a": user_a.to_string(),
        "user_b": user_b.to_string(),
        "status": "accepted",
    });

    state
        .supabase
        .insert("friends", insert_body)
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    Ok(Json(json!({ "status": "accepted" })))
}

// ---------------------------------------------------------------------------
// GET /friends
// ---------------------------------------------------------------------------

pub async fn get_friends_handler(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<impl IntoResponse, ApiError> {
    let me = get_session(&cookies)?;
    let me_str = me.to_string();

    // Fetch rows where I am user_a.
    let rows_a = state
        .supabase
        .select("friends")
        .eq("user_a", &me_str)
        .eq("status", "accepted")
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    // Fetch rows where I am user_b.
    let rows_b = state
        .supabase
        .select("friends")
        .eq("user_b", &me_str)
        .eq("status", "accepted")
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    let mut friend_ids: Vec<Uuid> = Vec::new();

    // From rows where I am user_a, the friend is user_b.
    for row_val in &rows_a {
        if let Ok(row) = serde_json::from_value::<FriendRow>(row_val.clone()) {
            friend_ids.push(row.user_b);
        }
    }

    // From rows where I am user_b, the friend is user_a.
    for row_val in &rows_b {
        if let Ok(row) = serde_json::from_value::<FriendRow>(row_val.clone()) {
            friend_ids.push(row.user_a);
        }
    }

    // Resolve each friend id into a FriendInfo with profile details.
    let mut friends: Vec<FriendInfo> = Vec::new();

    for fid in &friend_ids {
        let profile_result = state
            .supabase
            .select("profiles")
            .eq("id", &fid.to_string())
            .execute()
            .await;

        if let Ok(profile_rows) = profile_result {
            if let Some(first) = profile_rows.into_iter().next() {
                if let Ok(p) = serde_json::from_value::<ProfileRow>(first) {
                    friends.push(FriendInfo {
                        friend_id: p.id,
                        username: p.username,
                        display_name: p.display_name,
                        avatar_url: p.avatar_url,
                        status: "accepted".into(),
                    });
                }
            }
        }
    }

    Ok(Json(json!({ "friends": friends })))
}

// ---------------------------------------------------------------------------
// GET /friends/pending
// ---------------------------------------------------------------------------

pub async fn get_pending_friends_handler(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<impl IntoResponse, ApiError> {
    let me = get_session(&cookies)?;
    let me_str = me.to_string();

    // Pending requests where I am user_a.
    let rows_a = state
        .supabase
        .select("friends")
        .eq("user_a", &me_str)
        .eq("status", "pending")
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    // Pending requests where I am user_b.
    let rows_b = state
        .supabase
        .select("friends")
        .eq("user_b", &me_str)
        .eq("status", "pending")
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    let mut pending: Vec<FriendInfo> = Vec::new();

    for row_val in &rows_a {
        if let Ok(row) = serde_json::from_value::<FriendRow>(row_val.clone()) {
            if let Ok(p) = fetch_profile_brief(&state, row.user_b).await {
                pending.push(p);
            }
        }
    }

    for row_val in &rows_b {
        if let Ok(row) = serde_json::from_value::<FriendRow>(row_val.clone()) {
            if let Ok(p) = fetch_profile_brief(&state, row.user_a).await {
                pending.push(p);
            }
        }
    }

    Ok(Json(json!({ "pending": pending })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fetch minimal profile info for a friend entry.
async fn fetch_profile_brief(state: &AppState, user_id: Uuid) -> Result<FriendInfo, ApiError> {
    let rows = state
        .supabase
        .select("profiles")
        .eq("id", &user_id.to_string())
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    if rows.is_empty() {
        return Err(ApiError::NotFound("Profile not found".into()));
    }

    let p: ProfileRow =
        serde_json::from_value(rows[0].clone()).map_err(|e| ApiError::Database(e.to_string()))?;

    Ok(FriendInfo {
        friend_id: p.id,
        username: p.username,
        display_name: p.display_name,
        avatar_url: p.avatar_url,
        status: "pending".into(),
    })
}
