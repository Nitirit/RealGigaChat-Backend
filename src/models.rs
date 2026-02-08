use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    /// Optional display name; defaults to username if omitted.
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub user_id: Uuid,
    pub username: String,
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// The full profile row as stored in Supabase.
/// `password_hash` is only used server-side and is never sent to the client.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProfileRow {
    pub id: Uuid,
    pub username: String,
    #[serde(default)]
    pub password_hash: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub bio: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// The public-facing profile returned to clients (no password hash).
#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    pub id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub created_at: Option<String>,
}

impl From<ProfileRow> for ProfileResponse {
    fn from(row: ProfileRow) -> Self {
        Self {
            id: row.id,
            username: row.username,
            display_name: row.display_name,
            avatar_url: row.avatar_url,
            bio: row.bio,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct EditProfileRequest {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
}

// ---------------------------------------------------------------------------
// Friends
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AddFriendRequest {
    /// The UUID of the user to add as a friend.
    pub friend_id: Uuid,
}

/// Matches the actual Supabase `friends` table.
/// `id` is int8 (auto-increment bigint), not UUID.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FriendRow {
    #[serde(default)]
    pub id: Option<i64>,
    pub user_a: Uuid,
    pub user_b: Uuid,
    pub status: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FriendInfo {
    pub friend_id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status: String,
}

// ---------------------------------------------------------------------------
// Conversations & Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StartConversationRequest {
    /// The user to start a conversation with.
    pub friend_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    pub conversation_id: Uuid,
}

/// Matches the actual Supabase `messages` table.
/// `id` is int8 (auto-increment bigint), not UUID.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MessageRow {
    #[serde(default)]
    pub id: Option<i64>,
    pub conversation_id: Uuid,
    pub sender_id: Uuid,
    pub content: String,
    #[serde(default)]
    pub message_type: Option<String>,
    #[serde(default)]
    pub is_deleted: Option<bool>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// What the WebSocket client sends.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct WsIncoming {
    pub content: String,
}

/// What the server broadcasts to everyone in the conversation.
#[derive(Debug, Serialize, Clone)]
pub struct WsBroadcast {
    pub sender_id: Uuid,
    pub content: String,
    pub created_at: String,
}
