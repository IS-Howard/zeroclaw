//! LINE Messaging API channel.
//!
//! This channel operates in webhook mode (push-based). Messages are received
//! via the gateway's `/line` webhook endpoint. The `listen` method is a
//! keep-alive no-op; actual inbound handling happens in the gateway.

use super::line_markdown;
use super::line_templates;
use super::traits::{Channel, ChannelMessage, SendMessage};
use crate::config::schema::{LineDmPolicy, LineGroupOverride, LineGroupPolicy};
use anyhow::{bail, Context};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

const LINE_API_BASE: &str = "https://api.line.me";
const LINE_DATA_API_BASE: &str = "https://api-data.line.me";

/// Maximum text message length (LINE limit).
const MAX_TEXT_LENGTH: usize = 5000;

/// Maximum messages per push/reply batch (LINE limit).
const MAX_MESSAGES_PER_BATCH: usize = 5;

/// Profile cache TTL.
const PROFILE_CACHE_TTL_SECS: u64 = 300; // 5 minutes

/// Cached LINE user profile.
#[derive(Debug, Clone)]
pub struct LineUserProfile {
    pub display_name: String,
    pub picture_url: Option<String>,
}

/// LINE Messaging API channel.
pub struct LineChannel {
    channel_access_token: String,
    channel_secret: String,
    allowed_users: Vec<String>,
    allowed_groups: Vec<String>,
    dm_policy: LineDmPolicy,
    group_policy: LineGroupPolicy,
    mention_only: bool,
    media_max_bytes: usize,
    groups: HashMap<String, LineGroupOverride>,
    profile_cache: Arc<Mutex<HashMap<String, (LineUserProfile, Instant)>>>,
    http: reqwest::Client,
}

impl LineChannel {
    pub fn new(
        channel_access_token: String,
        channel_secret: String,
        allowed_users: Vec<String>,
        allowed_groups: Vec<String>,
        dm_policy: LineDmPolicy,
        group_policy: LineGroupPolicy,
        mention_only: bool,
        media_max_bytes: usize,
        groups: HashMap<String, LineGroupOverride>,
    ) -> Self {
        Self {
            channel_access_token,
            channel_secret,
            allowed_users,
            allowed_groups,
            dm_policy,
            group_policy,
            mention_only,
            media_max_bytes,
            groups,
            profile_cache: Arc::new(Mutex::new(HashMap::new())),
            http: crate::config::build_runtime_proxy_client("channel.line"),
        }
    }

    /// Get the channel secret (used by gateway for signature verification).
    pub fn channel_secret(&self) -> &str {
        &self.channel_secret
    }

    // ─── Signature Verification ────────────────────────────────────────

    /// Verify LINE webhook signature.
    ///
    /// LINE uses HMAC-SHA256 of the request body with the channel secret,
    /// encoded as **base64** (not hex like WhatsApp).
    pub fn verify_signature(channel_secret: &str, body: &[u8], signature_header: &str) -> bool {
        use base64::Engine;
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(channel_secret.as_bytes()) else {
            return false;
        };
        mac.update(body);
        let computed = mac.finalize().into_bytes();

        let Ok(expected) = base64::engine::general_purpose::STANDARD.decode(signature_header)
        else {
            return false;
        };

        // Constant-time comparison
        if computed.len() != expected.len() {
            return false;
        }
        let mut diff = 0u8;
        for (a, b) in computed.iter().zip(expected.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }

    // ─── Webhook Payload Parsing ───────────────────────────────────────

    /// Parse an incoming LINE webhook payload and extract messages.
    ///
    /// Applies access control (dm_policy, group_policy, allowlists).
    pub fn parse_webhook_payload(&self, payload: &Value) -> Vec<ChannelMessage> {
        let mut messages = Vec::new();

        let Some(events) = payload.get("events").and_then(|e| e.as_array()) else {
            return messages;
        };

        for event in events {
            let event_type = event
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("");

            let timestamp = event
                .get("timestamp")
                .and_then(|t| t.as_u64())
                .map(|ms| ms / 1000) // LINE timestamps are milliseconds
                .unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                });

            // Extract source info
            let source = event.get("source");
            let source_type = source
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("user");
            let user_id = source
                .and_then(|s| s.get("userId"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let group_id = source
                .and_then(|s| s.get("groupId"))
                .and_then(|g| g.as_str());
            let room_id = source
                .and_then(|s| s.get("roomId"))
                .and_then(|r| r.as_str());

            if user_id.is_empty() {
                continue;
            }

            let is_group = source_type == "group" || source_type == "room";

            // Access control
            if !self.check_access(user_id, group_id.or(room_id), is_group) {
                tracing::debug!(
                    "LINE: access denied for user {user_id} in {} context",
                    if is_group { "group" } else { "DM" }
                );
                continue;
            }

            // Determine reply target
            let reply_target = group_id
                .or(room_id)
                .unwrap_or(user_id)
                .to_string();

            match event_type {
                "message" => {
                    if let Some(content) = self.extract_message_content(event) {
                        if !content.is_empty() {
                            messages.push(ChannelMessage {
                                id: event
                                    .get("message")
                                    .and_then(|m| m.get("id"))
                                    .and_then(|id| id.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                sender: user_id.to_string(),
                                reply_target,
                                content,
                                channel: "line".to_string(),
                                timestamp,
                                thread_ts: None,
                            });
                        }
                    }
                }
                "postback" => {
                    let data = event
                        .get("postback")
                        .and_then(|p| p.get("data"))
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !data.is_empty() {
                        messages.push(ChannelMessage {
                            id: uuid::Uuid::new_v4().to_string(),
                            sender: user_id.to_string(),
                            reply_target,
                            content: data,
                            channel: "line".to_string(),
                            timestamp,
                            thread_ts: None,
                        });
                    }
                }
                "follow" => {
                    tracing::info!("LINE: user {user_id} followed the bot");
                }
                "unfollow" => {
                    tracing::info!("LINE: user {user_id} unfollowed the bot");
                }
                "join" => {
                    let target = group_id.or(room_id).unwrap_or("unknown");
                    tracing::info!("LINE: bot joined {source_type} {target}");
                }
                "leave" => {
                    let target = group_id.or(room_id).unwrap_or("unknown");
                    tracing::info!("LINE: bot left {source_type} {target}");
                }
                _ => {
                    tracing::debug!("LINE: ignoring event type: {event_type}");
                }
            }
        }

        messages
    }

    /// Extract message content from a LINE message event.
    fn extract_message_content(&self, event: &Value) -> Option<String> {
        let msg = event.get("message")?;
        let msg_type = msg.get("type")?.as_str()?;

        match msg_type {
            "text" => msg.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()),
            "location" => {
                let title = msg
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Location");
                let address = msg
                    .get("address")
                    .and_then(|a| a.as_str())
                    .unwrap_or("");
                let lat = msg.get("latitude").and_then(|l| l.as_f64()).unwrap_or(0.0);
                let lng = msg
                    .get("longitude")
                    .and_then(|l| l.as_f64())
                    .unwrap_or(0.0);
                Some(format!("[Location: {title} ({address}) at {lat},{lng}]"))
            }
            "sticker" => {
                let pkg = msg
                    .get("packageId")
                    .and_then(|p| p.as_str())
                    .unwrap_or("?");
                let stk = msg
                    .get("stickerId")
                    .and_then(|s| s.as_str())
                    .unwrap_or("?");
                Some(format!("[Sticker: {pkg}/{stk}]"))
            }
            "image" => {
                let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or("");
                Some(format!("[IMAGE:line-media:{msg_id}]"))
            }
            "video" => {
                let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or("");
                Some(format!("[VIDEO:line-media:{msg_id}]"))
            }
            "audio" => {
                let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or("");
                Some(format!("[AUDIO:line-media:{msg_id}]"))
            }
            "file" => {
                let filename = msg
                    .get("fileName")
                    .and_then(|f| f.as_str())
                    .unwrap_or("file");
                let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or("");
                Some(format!("[DOCUMENT:{filename}:line-media:{msg_id}]"))
            }
            _ => {
                tracing::debug!("LINE: unsupported message type: {msg_type}");
                None
            }
        }
    }

    // ─── Access Control ────────────────────────────────────────────────

    /// Check if a user/group is allowed based on configured policies.
    fn check_access(&self, user_id: &str, group_id: Option<&str>, is_group: bool) -> bool {
        if is_group {
            match self.group_policy {
                LineGroupPolicy::Disabled => return false,
                LineGroupPolicy::Allowlist => {
                    if let Some(gid) = group_id {
                        if !is_in_list(&self.allowed_groups, gid) {
                            return false;
                        }
                        // Check per-group override
                        if let Some(override_cfg) = self.groups.get(gid) {
                            if !override_cfg.enabled {
                                return false;
                            }
                            if !override_cfg.allowed_users.is_empty()
                                && !is_in_list(&override_cfg.allowed_users, user_id)
                            {
                                return false;
                            }
                        }
                    } else {
                        return false;
                    }
                }
                LineGroupPolicy::Open => {
                    // Check per-group override if it exists
                    if let Some(gid) = group_id {
                        if let Some(override_cfg) = self.groups.get(gid) {
                            if !override_cfg.enabled {
                                return false;
                            }
                        }
                    }
                }
            }
        } else {
            match self.dm_policy {
                LineDmPolicy::Disabled => return false,
                LineDmPolicy::Allowlist => {
                    if !is_in_list(&self.allowed_users, user_id) {
                        return false;
                    }
                }
                LineDmPolicy::Open => {}
            }
        }
        true
    }

    // ─── Outbound Sending ──────────────────────────────────────────────

    /// Process and send an outbound message through the LINE API.
    ///
    /// Pipeline:
    /// 1. Run markdown conversion (tables → Flex, code → Flex, strip formatting)
    /// 2. Parse special markers ([[quick_replies:...]], [[location:...]], etc.)
    /// 3. Batch messages (max 5 per push request)
    async fn send_message(&self, recipient: &str, content: &str) -> anyhow::Result<()> {
        // Normalize recipient (strip line: prefixes)
        let to = normalize_target(recipient);

        let mut all_messages: Vec<Value> = Vec::new();

        // 1. Process markdown → plain text + flex messages
        let processed = line_markdown::process_line_message(content);

        // 2. Add flex messages
        for flex in processed.flex_messages {
            all_messages.push(flex);
        }

        // 3. Parse special markers from text
        let (text, quick_replies, location, templates) =
            parse_special_markers(&processed.text);

        // 4. Add template messages
        for tmpl in templates {
            all_messages.push(tmpl);
        }

        // 5. Add location message
        if let Some(loc) = location {
            all_messages.push(loc);
        }

        // 6. Chunk text and add text messages
        let text_chunks = chunk_text(&text, MAX_TEXT_LENGTH);
        for (i, chunk) in text_chunks.iter().enumerate() {
            if chunk.trim().is_empty() {
                continue;
            }
            let mut msg = json!({
                "type": "text",
                "text": chunk,
            });

            // Attach quick replies to last text message only
            if i == text_chunks.len() - 1 && !quick_replies.is_empty() {
                line_templates::attach_quick_replies(&mut msg, &quick_replies);
            }

            all_messages.push(msg);
        }

        // If only quick replies and no other content, send a prompt message
        if all_messages.is_empty() && !quick_replies.is_empty() {
            let mut msg = json!({
                "type": "text",
                "text": "Please select:",
            });
            line_templates::attach_quick_replies(&mut msg, &quick_replies);
            all_messages.push(msg);
        }

        if all_messages.is_empty() {
            return Ok(());
        }

        // 7. Send in batches of MAX_MESSAGES_PER_BATCH
        for batch in all_messages.chunks(MAX_MESSAGES_PER_BATCH) {
            self.push_messages(&to, batch).await?;
        }

        Ok(())
    }

    /// Push messages via LINE Messaging API.
    async fn push_messages(&self, to: &str, messages: &[Value]) -> anyhow::Result<()> {
        let url = format!("{LINE_API_BASE}/v2/bot/message/push");
        let body = json!({
            "to": to,
            "messages": messages,
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.channel_access_token)
            .json(&body)
            .send()
            .await
            .context("LINE push message request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let error_body = resp.text().await.unwrap_or_default();
            let sanitized = crate::providers::sanitize_api_error(&error_body);
            tracing::error!("LINE push failed: {status} — {sanitized}");
            bail!("LINE API error: {status}");
        }

        Ok(())
    }

    /// Reply to a specific event using a reply token.
    pub async fn reply_messages(
        &self,
        reply_token: &str,
        messages: &[Value],
    ) -> anyhow::Result<()> {
        let url = format!("{LINE_API_BASE}/v2/bot/message/reply");
        let body = json!({
            "replyToken": reply_token,
            "messages": messages,
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.channel_access_token)
            .json(&body)
            .send()
            .await
            .context("LINE reply message request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let error_body = resp.text().await.unwrap_or_default();
            let sanitized = crate::providers::sanitize_api_error(&error_body);
            tracing::error!("LINE reply failed: {status} — {sanitized}");
            bail!("LINE reply API error: {status}");
        }

        Ok(())
    }

    // ─── Profile Cache ─────────────────────────────────────────────────

    /// Get a user's display name, with 5-minute cache.
    pub async fn get_user_profile(&self, user_id: &str) -> anyhow::Result<LineUserProfile> {
        // Check cache
        {
            let cache = self.profile_cache.lock();
            if let Some((profile, fetched_at)) = cache.get(user_id) {
                if fetched_at.elapsed().as_secs() < PROFILE_CACHE_TTL_SECS {
                    return Ok(profile.clone());
                }
            }
        }

        // Fetch from API
        let url = format!("{LINE_API_BASE}/v2/bot/profile/{user_id}");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.channel_access_token)
            .send()
            .await
            .context("LINE get profile request failed")?;

        if !resp.status().is_success() {
            bail!("LINE profile API returned {}", resp.status());
        }

        let body: Value = resp.json().await?;
        let profile = LineUserProfile {
            display_name: body
                .get("displayName")
                .and_then(|d| d.as_str())
                .unwrap_or("Unknown")
                .to_string(),
            picture_url: body
                .get("pictureUrl")
                .and_then(|p| p.as_str())
                .map(|s| s.to_string()),
        };

        // Update cache
        {
            let mut cache = self.profile_cache.lock();
            cache.insert(user_id.to_string(), (profile.clone(), Instant::now()));
        }

        Ok(profile)
    }

    /// Show loading animation in chat.
    pub async fn show_loading(&self, chat_id: &str) -> anyhow::Result<()> {
        let url = format!("{LINE_API_BASE}/v2/bot/chat/loading");
        let body = json!({ "chatId": chat_id });

        let _ = self
            .http
            .post(&url)
            .bearer_auth(&self.channel_access_token)
            .json(&body)
            .send()
            .await;

        Ok(())
    }
}

#[async_trait]
impl Channel for LineChannel {
    fn name(&self) -> &str {
        "line"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        self.send_message(&message.recipient, &message.content)
            .await
    }

    async fn listen(
        &self,
        _tx: tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        // LINE uses webhooks (push-based), not polling.
        // Messages are received via the gateway's /line endpoint.
        tracing::info!(
            "LINE channel active (webhook mode). \
            Configure LINE webhook URL to POST to your gateway's /line endpoint."
        );

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    }

    async fn health_check(&self) -> bool {
        let url = format!("{LINE_API_BASE}/v2/bot/info");
        self.http
            .get(&url)
            .bearer_auth(&self.channel_access_token)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    async fn start_typing(&self, recipient: &str) -> anyhow::Result<()> {
        let to = normalize_target(recipient);
        self.show_loading(&to).await
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Normalize a LINE target ID by stripping common prefixes.
fn normalize_target(target: &str) -> String {
    target
        .strip_prefix("line:")
        .or_else(|| target.strip_prefix("line:user:"))
        .or_else(|| target.strip_prefix("line:group:"))
        .or_else(|| target.strip_prefix("line:room:"))
        .unwrap_or(target)
        .to_string()
}

/// Check if an ID is in an allowlist (supports `"*"` wildcard).
fn is_in_list(list: &[String], id: &str) -> bool {
    list.iter().any(|item| item == "*" || item == id)
}

/// Chunk text into segments of at most `max_len` characters.
fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    if text.chars().count() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut char_count = 0;

    for ch in text.chars() {
        if char_count >= max_len {
            chunks.push(current);
            current = String::new();
            char_count = 0;
        }
        current.push(ch);
        char_count += 1;
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Parse special markers from text and extract them.
///
/// Supported markers:
/// - `[[quick_replies: Option 1, Option 2, Option 3]]`
/// - `[[location: Title | Address | lat | lng]]`
/// - `[[confirm: Question? | Yes Label | No Label]]`
/// - `[[buttons: Title | Text | Btn1:action1, Btn2:action2]]`
fn parse_special_markers(text: &str) -> (String, Vec<String>, Option<Value>, Vec<Value>) {
    let mut remaining = text.to_string();
    let mut quick_replies = Vec::new();
    let mut location = None;
    let mut templates = Vec::new();

    // Quick replies: [[quick_replies: opt1, opt2, ...]]
    if let Some(caps) = regex::Regex::new(r"\[\[quick_replies:\s*(.+?)\]\]")
        .ok()
        .and_then(|re| re.captures(&remaining))
    {
        let opts = caps[1].split(',').map(|s| s.trim().to_string()).collect();
        quick_replies = opts;
        remaining = regex::Regex::new(r"\[\[quick_replies:\s*.+?\]\]")
            .unwrap()
            .replace_all(&remaining, "")
            .to_string();
    }

    // Location: [[location: Title | Address | lat | lng]]
    if let Some(caps) = regex::Regex::new(r"\[\[location:\s*(.+?)\]\]")
        .ok()
        .and_then(|re| re.captures(&remaining))
    {
        let parts: Vec<&str> = caps[1].split('|').map(|s| s.trim()).collect();
        if parts.len() >= 4 {
            if let (Ok(lat), Ok(lng)) = (
                parts[2].parse::<f64>(),
                parts[3].parse::<f64>(),
            ) {
                location = Some(json!({
                    "type": "location",
                    "title": parts[0],
                    "address": parts[1],
                    "latitude": lat,
                    "longitude": lng,
                }));
            }
        }
        remaining = regex::Regex::new(r"\[\[location:\s*.+?\]\]")
            .unwrap()
            .replace_all(&remaining, "")
            .to_string();
    }

    // Confirm: [[confirm: Question? | Yes | No]]
    if let Some(caps) = regex::Regex::new(r"\[\[confirm:\s*(.+?)\]\]")
        .ok()
        .and_then(|re| re.captures(&remaining))
    {
        let parts: Vec<&str> = caps[1].split('|').map(|s| s.trim()).collect();
        if parts.len() >= 3 {
            let tmpl = line_templates::create_confirm_template(
                parts[0],
                line_templates::TemplateAction::Message {
                    label: parts[1].to_string(),
                    text: parts[1].to_string(),
                },
                line_templates::TemplateAction::Message {
                    label: parts[2].to_string(),
                    text: parts[2].to_string(),
                },
                None,
            );
            templates.push(tmpl);
        }
        remaining = regex::Regex::new(r"\[\[confirm:\s*.+?\]\]")
            .unwrap()
            .replace_all(&remaining, "")
            .to_string();
    }

    // Buttons: [[buttons: Title | Text | Btn1:action1, Btn2:action2]]
    if let Some(caps) = regex::Regex::new(r"\[\[buttons:\s*(.+?)\]\]")
        .ok()
        .and_then(|re| re.captures(&remaining))
    {
        let parts: Vec<&str> = caps[1].split('|').map(|s| s.trim()).collect();
        if parts.len() >= 3 {
            let actions: Vec<line_templates::TemplateAction> = parts[2]
                .split(',')
                .map(|s| line_templates::TemplateAction::parse(s.trim()))
                .collect();
            let tmpl =
                line_templates::create_button_template(parts[0], parts[1], &actions, None);
            templates.push(tmpl);
        }
        remaining = regex::Regex::new(r"\[\[buttons:\s*.+?\]\]")
            .unwrap()
            .replace_all(&remaining, "")
            .to_string();
    }

    (remaining, quick_replies, location, templates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel() -> LineChannel {
        LineChannel::new(
            "test-token".into(),
            "test-secret".into(),
            vec!["U001".into()],
            vec!["C001".into()],
            LineDmPolicy::Allowlist,
            LineGroupPolicy::Allowlist,
            false,
            10 * 1024 * 1024,
            HashMap::new(),
        )
    }

    fn make_open_channel() -> LineChannel {
        LineChannel::new(
            "test-token".into(),
            "test-secret".into(),
            vec![],
            vec![],
            LineDmPolicy::Open,
            LineGroupPolicy::Open,
            false,
            10 * 1024 * 1024,
            HashMap::new(),
        )
    }

    // ── Signature ──────────────────────────────────────────────────────

    #[test]
    fn verify_valid_signature() {
        use base64::Engine;
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let secret = "test-secret";
        let body = b"test body";

        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

        assert!(LineChannel::verify_signature(secret, body, &sig));
    }

    #[test]
    fn reject_invalid_signature() {
        assert!(!LineChannel::verify_signature(
            "secret",
            b"body",
            "invalid-base64!"
        ));
    }

    #[test]
    fn reject_wrong_signature() {
        use base64::Engine;
        let wrong = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 32]);
        assert!(!LineChannel::verify_signature("secret", b"body", &wrong));
    }

    #[test]
    fn reject_empty_signature() {
        assert!(!LineChannel::verify_signature("secret", b"body", ""));
    }

    // ── Webhook Parsing ────────────────────────────────────────────────

    #[test]
    fn parse_text_message() {
        let ch = make_open_channel();
        let payload = json!({
            "events": [{
                "type": "message",
                "timestamp": 1700000000000u64,
                "source": { "type": "user", "userId": "U123" },
                "replyToken": "tok",
                "message": { "id": "msg1", "type": "text", "text": "Hello!" }
            }]
        });

        let msgs = ch.parse_webhook_payload(&payload);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].sender, "U123");
        assert_eq!(msgs[0].content, "Hello!");
        assert_eq!(msgs[0].channel, "line");
        assert_eq!(msgs[0].timestamp, 1_700_000_000);
    }

    #[test]
    fn parse_location_message() {
        let ch = make_open_channel();
        let payload = json!({
            "events": [{
                "type": "message",
                "timestamp": 1700000000000u64,
                "source": { "type": "user", "userId": "U123" },
                "message": {
                    "id": "msg2",
                    "type": "location",
                    "title": "Office",
                    "address": "123 Main St",
                    "latitude": 35.6762,
                    "longitude": 139.6503
                }
            }]
        });

        let msgs = ch.parse_webhook_payload(&payload);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].content.contains("Office"));
        assert!(msgs[0].content.contains("35.6762"));
    }

    #[test]
    fn parse_sticker_message() {
        let ch = make_open_channel();
        let payload = json!({
            "events": [{
                "type": "message",
                "timestamp": 1700000000000u64,
                "source": { "type": "user", "userId": "U123" },
                "message": { "id": "msg3", "type": "sticker", "packageId": "1", "stickerId": "100" }
            }]
        });

        let msgs = ch.parse_webhook_payload(&payload);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "[Sticker: 1/100]");
    }

    #[test]
    fn parse_postback_event() {
        let ch = make_open_channel();
        let payload = json!({
            "events": [{
                "type": "postback",
                "timestamp": 1700000000000u64,
                "source": { "type": "user", "userId": "U123" },
                "postback": { "data": "action=buy&item=1" }
            }]
        });

        let msgs = ch.parse_webhook_payload(&payload);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "action=buy&item=1");
    }

    #[test]
    fn parse_group_message() {
        let ch = make_open_channel();
        let payload = json!({
            "events": [{
                "type": "message",
                "timestamp": 1700000000000u64,
                "source": { "type": "group", "userId": "U123", "groupId": "C456" },
                "message": { "id": "msg4", "type": "text", "text": "group msg" }
            }]
        });

        let msgs = ch.parse_webhook_payload(&payload);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].reply_target, "C456");
    }

    #[test]
    fn parse_empty_payload() {
        let ch = make_channel();
        assert!(ch.parse_webhook_payload(&json!({})).is_empty());
    }

    #[test]
    fn parse_no_events() {
        let ch = make_channel();
        assert!(ch
            .parse_webhook_payload(&json!({"events": []}))
            .is_empty());
    }

    // ── Access Control ─────────────────────────────────────────────────

    #[test]
    fn dm_allowlist_allows_listed_user() {
        let ch = make_channel();
        assert!(ch.check_access("U001", None, false));
    }

    #[test]
    fn dm_allowlist_denies_unlisted_user() {
        let ch = make_channel();
        assert!(!ch.check_access("U999", None, false));
    }

    #[test]
    fn group_allowlist_allows_listed_group() {
        let ch = make_channel();
        assert!(ch.check_access("U001", Some("C001"), true));
    }

    #[test]
    fn group_allowlist_denies_unlisted_group() {
        let ch = make_channel();
        assert!(!ch.check_access("U001", Some("C999"), true));
    }

    #[test]
    fn dm_disabled_denies_all() {
        let ch = LineChannel::new(
            "tok".into(),
            "sec".into(),
            vec![],
            vec![],
            LineDmPolicy::Disabled,
            LineGroupPolicy::Open,
            false,
            10_000_000,
            HashMap::new(),
        );
        assert!(!ch.check_access("U001", None, false));
    }

    #[test]
    fn open_policy_allows_all() {
        let ch = make_open_channel();
        assert!(ch.check_access("anyone", None, false));
        assert!(ch.check_access("anyone", Some("any_group"), true));
    }

    #[test]
    fn wildcard_allowlist() {
        let ch = LineChannel::new(
            "tok".into(),
            "sec".into(),
            vec!["*".into()],
            vec!["*".into()],
            LineDmPolicy::Allowlist,
            LineGroupPolicy::Allowlist,
            false,
            10_000_000,
            HashMap::new(),
        );
        assert!(ch.check_access("anyone", None, false));
        assert!(ch.check_access("anyone", Some("any_group"), true));
    }

    #[test]
    fn access_denied_filters_messages() {
        let ch = make_channel();
        let payload = json!({
            "events": [{
                "type": "message",
                "timestamp": 1700000000000u64,
                "source": { "type": "user", "userId": "U999" },
                "message": { "id": "msg", "type": "text", "text": "blocked" }
            }]
        });
        assert!(ch.parse_webhook_payload(&payload).is_empty());
    }

    // ── Text Chunking ──────────────────────────────────────────────────

    #[test]
    fn chunk_short_text() {
        let chunks = chunk_text("hello", 5000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello");
    }

    #[test]
    fn chunk_long_text() {
        let text = "a".repeat(12000);
        let chunks = chunk_text(&text, 5000);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 5000);
        assert_eq!(chunks[1].len(), 5000);
        assert_eq!(chunks[2].len(), 2000);
    }

    #[test]
    fn chunk_empty_text() {
        assert!(chunk_text("", 5000).is_empty());
    }

    // ── Target Normalization ───────────────────────────────────────────

    #[test]
    fn normalize_plain_id() {
        assert_eq!(normalize_target("U123"), "U123");
    }

    #[test]
    fn normalize_prefixed_id() {
        assert_eq!(normalize_target("line:U123"), "U123");
        assert_eq!(normalize_target("line:user:U123"), "U123");
        assert_eq!(normalize_target("line:group:C456"), "C456");
    }

    // ── Special Markers ────────────────────────────────────────────────

    #[test]
    fn parse_quick_replies_marker() {
        let (text, qr, _, _) =
            parse_special_markers("Hello [[quick_replies: Yes, No, Maybe]]");
        assert_eq!(qr, vec!["Yes", "No", "Maybe"]);
        assert!(!text.contains("quick_replies"));
    }

    #[test]
    fn parse_location_marker() {
        let (_, _, loc, _) =
            parse_special_markers("[[location: Office | 123 Main St | 35.6 | 139.7]]");
        assert!(loc.is_some());
        let loc = loc.unwrap();
        assert_eq!(loc["title"], "Office");
    }

    #[test]
    fn parse_confirm_marker() {
        let (_, _, _, templates) =
            parse_special_markers("[[confirm: Delete? | Yes | No]]");
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0]["template"]["type"], "confirm");
    }

    #[test]
    fn parse_buttons_marker() {
        let (_, _, _, templates) =
            parse_special_markers("[[buttons: Menu | Choose | Opt1:val1, Opt2:val2]]");
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0]["template"]["type"], "buttons");
    }

    #[test]
    fn no_markers_passthrough() {
        let (text, qr, loc, tmpl) = parse_special_markers("Just plain text");
        assert_eq!(text, "Just plain text");
        assert!(qr.is_empty());
        assert!(loc.is_none());
        assert!(tmpl.is_empty());
    }
}
