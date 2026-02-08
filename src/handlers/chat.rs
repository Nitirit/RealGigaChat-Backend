use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tower_cookies::Cookies;
use tracing::{error, info};
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::auth::get_session;
use crate::models::{ConversationResponse, MessageRow, StartConversationRequest, WsBroadcast};
use crate::AppState;

/// Type alias for the shared map of conversation broadcast channels.
pub type ConversationChannels = Arc<RwLock<HashMap<Uuid, broadcast::Sender<WsBroadcast>>>>;

/// Create a new empty channel map. Called once at startup.
pub fn new_channel_map() -> ConversationChannels {
    Arc::new(RwLock::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// POST /conversations  –  start (or retrieve) a 1-on-1 conversation
// ---------------------------------------------------------------------------

pub async fn start_conversation_handler(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(body): Json<StartConversationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let me = get_session(&cookies)?;
    info!(
        "[start_conversation] me={}, friend_id={}",
        me, body.friend_id
    );

    if me == body.friend_id {
        return Err(ApiError::BadRequest(
            "Cannot start a conversation with yourself".into(),
        ));
    }

    // Check if a conversation already exists between these two users.
    // We query conversation_members for both users and find a shared conversation_id.
    let my_convos = state
        .supabase
        .select("conversation_members")
        .eq("user_id", &me.to_string())
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    let friend_convos = state
        .supabase
        .select("conversation_members")
        .eq("user_id", &body.friend_id.to_string())
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    let my_ids = extract_conversation_ids(&my_convos);
    let friend_ids = extract_conversation_ids(&friend_convos);

    // Find any conversation_id that appears in both sets.
    for cid in &my_ids {
        if friend_ids.contains(cid) {
            return Ok(Json(ConversationResponse {
                conversation_id: *cid,
            }));
        }
    }

    // No existing conversation – create one.
    let conv_id = Uuid::new_v4();
    info!(
        "[start_conversation] Creating new conversation: {}",
        conv_id
    );

    state
        .supabase
        .insert(
            "conversations",
            json!({
                "id": conv_id.to_string(),
                "is_group": false,
            }),
        )
        .await
        .map_err(|e| {
            error!("[start_conversation] Failed to insert conversation: {}", e);
            ApiError::Database(e.to_string())
        })?;

    // Add both users as members (include "role" column from actual schema).
    state
        .supabase
        .insert(
            "conversation_members",
            json!({
                "conversation_id": conv_id.to_string(),
                "user_id": me.to_string(),
                "role": "member",
            }),
        )
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    state
        .supabase
        .insert(
            "conversation_members",
            json!({
                "conversation_id": conv_id.to_string(),
                "user_id": body.friend_id.to_string(),
                "role": "member",
            }),
        )
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    info!(
        "[start_conversation] Success, returning conversation_id: {}",
        conv_id
    );
    Ok(Json(ConversationResponse {
        conversation_id: conv_id,
    }))
}

// ---------------------------------------------------------------------------
// GET /conversations  –  list my conversations
// ---------------------------------------------------------------------------

pub async fn list_conversations_handler(
    State(state): State<AppState>,
    cookies: Cookies,
) -> Result<impl IntoResponse, ApiError> {
    let me = get_session(&cookies)?;

    let result = state
        .supabase
        .select("conversation_members")
        .eq("user_id", &me.to_string())
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    let ids = extract_conversation_ids(&result);
    Ok(Json(json!({ "conversations": ids })))
}

// ---------------------------------------------------------------------------
// GET /conversations/{id}/messages  –  fetch message history
// ---------------------------------------------------------------------------

pub async fn get_messages_handler(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    cookies: Cookies,
) -> Result<impl IntoResponse, ApiError> {
    info!("[get_messages] conversation_id={}", conversation_id);

    let me = get_session(&cookies)?;
    info!("[get_messages] user_id={}", me);

    // Verify the user is a member of this conversation.
    if let Err(e) = verify_membership(&state, conversation_id, me).await {
        error!("[get_messages] Membership verification failed: {:?}", e);
        return Err(e);
    }

    let rows = state
        .supabase
        .select("messages")
        .eq("conversation_id", &conversation_id.to_string())
        .execute()
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    let mut messages: Vec<MessageRow> = Vec::new();
    for val in rows {
        if let Ok(msg) = serde_json::from_value::<MessageRow>(val) {
            messages.push(msg);
        }
    }

    // Sort by created_at ascending so the client gets chronological order.
    messages.sort_by(|a, b| {
        let a_time = a.created_at.as_deref().unwrap_or("");
        let b_time = b.created_at.as_deref().unwrap_or("");
        a_time.cmp(b_time)
    });

    Ok(Json(json!({ "messages": messages })))
}

// ---------------------------------------------------------------------------
// GET /ws/{conversation_id}  –  WebSocket upgrade
// ---------------------------------------------------------------------------

pub async fn ws_handler(
    State(state): State<AppState>,
    Path(conversation_id): Path<Uuid>,
    cookies: Cookies,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = get_session(&cookies)?;

    // Verify membership before upgrading.
    verify_membership(&state, conversation_id, user_id).await?;

    let channels = state.channels.clone();
    let supabase = state.supabase.clone();

    Ok(ws.on_upgrade(move |socket| {
        handle_socket(socket, conversation_id, user_id, channels, supabase)
    }))
}

// ---------------------------------------------------------------------------
// WebSocket connection handler
// ---------------------------------------------------------------------------

async fn handle_socket(
    socket: WebSocket,
    conversation_id: Uuid,
    user_id: Uuid,
    channels: ConversationChannels,
    supabase: Arc<supabase_rs::SupabaseClient>,
) {
    // Split the socket into sender and receiver halves using futures-util.
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Get or create the broadcast channel for this conversation.
    let tx = {
        let mut map = channels.write().await;
        map.entry(conversation_id)
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(256);
                tx
            })
            .clone()
    };

    let mut rx = tx.subscribe();

    // Spawn a task that forwards broadcast messages → WebSocket sender.
    let mut send_task = tokio::spawn(async move {
        while let Ok(broadcast_msg) = rx.recv().await {
            let json_text = match serde_json::to_string(&broadcast_msg) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if ws_sender
                .send(Message::Text(json_text.into()))
                .await
                .is_err()
            {
                // Client disconnected.
                break;
            }
        }
    });

    // Main loop: read messages from the WebSocket client using StreamExt::next().
    let tx_for_recv = tx.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(result) = ws_receiver.next().await {
            let msg = match result {
                Ok(m) => m,
                Err(_) => break, // Connection error → stop.
            };

            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => break,
                _ => continue, // Ignore binary, ping, pong.
            };

            // Parse the incoming message. Expect: { "content": "..." }
            let content = match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(val) => match val.get("content").and_then(|c| c.as_str()) {
                    Some(c) => c.to_string(),
                    None => continue, // Ignore malformed messages.
                },
                Err(_) => {
                    // If it's not JSON, treat the raw text as the message content.
                    text.clone()
                }
            };

            if content.trim().is_empty() {
                continue;
            }

            let now = chrono::Utc::now().to_rfc3339();

            // Persist the message to Supabase (best-effort; don't kill the socket on failure).
            // Don't send "id" — it's auto-increment int8 in the actual schema.
            let insert_body = serde_json::json!({
                "conversation_id": conversation_id.to_string(),
                "sender_id": user_id.to_string(),
                "content": content,
                "message_type": "text",
            });

            let _ = supabase.insert("messages", insert_body).await;

            // Broadcast to all connected clients in this conversation.
            let broadcast_msg = WsBroadcast {
                sender_id: user_id,
                content,
                created_at: now,
            };

            // If nobody is listening the send will error, which is fine.
            let _ = tx_for_recv.send(broadcast_msg);
        }
    });

    // Wait for either task to finish, then abort the other.
    tokio::select! {
        _ = &mut send_task => {
            recv_task.abort();
        }
        _ = &mut recv_task => {
            send_task.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract conversation_id UUIDs from a Vec of conversation_members rows.
fn extract_conversation_ids(rows: &[serde_json::Value]) -> Vec<Uuid> {
    let mut ids = Vec::new();
    for row in rows {
        if let Some(cid_str) = row.get("conversation_id").and_then(|v| v.as_str()) {
            if let Ok(cid) = Uuid::parse_str(cid_str) {
                ids.push(cid);
            }
        }
    }
    ids
}

/// Check that the given user is a member of the conversation. Returns an error if not.
async fn verify_membership(
    state: &AppState,
    conversation_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    info!(
        "[verify_membership] conversation_id={}, user_id={}",
        conversation_id, user_id
    );

    let rows = state
        .supabase
        .select("conversation_members")
        .eq("conversation_id", &conversation_id.to_string())
        .eq("user_id", &user_id.to_string())
        .execute()
        .await
        .map_err(|e| {
            error!("[verify_membership] Database error: {}", e);
            ApiError::Database(e.to_string())
        })?;

    info!("[verify_membership] Found {} membership rows", rows.len());

    if rows.is_empty() {
        error!(
            "[verify_membership] User {} is not a member of conversation {}",
            user_id, conversation_id
        );
        return Err(ApiError::Unauthorized);
    }

    Ok(())
}
