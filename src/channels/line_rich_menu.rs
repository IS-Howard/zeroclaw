//! LINE Rich Menu management.
//!
//! Rich menus are persistent menus displayed at the bottom of the LINE chat.
//! This module provides CRUD operations and layout helpers.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

const LINE_API_BASE: &str = "https://api.line.me";
const RICH_MENU_BATCH_LIMIT: usize = 500;

// ─── Types ─────────────────────────────────────────────────────────────────

/// Rich menu size (LINE requires width=2500).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichMenuSize {
    pub width: u32,
    pub height: u32,
}

impl RichMenuSize {
    /// Full-height menu (2500×1686).
    pub fn full() -> Self {
        Self {
            width: 2500,
            height: 1686,
        }
    }

    /// Half-height menu (2500×843).
    pub fn half() -> Self {
        Self {
            width: 2500,
            height: 843,
        }
    }
}

/// Bounding rectangle for a rich menu tap area.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichMenuBounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A tap area in a rich menu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichMenuArea {
    pub bounds: RichMenuBounds,
    pub action: serde_json::Value,
}

/// Parameters for creating a rich menu.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRichMenuParams {
    pub size: RichMenuSize,
    #[serde(default)]
    pub selected: bool,
    pub name: String,
    pub chat_bar_text: String,
    pub areas: Vec<RichMenuArea>,
}

/// Response from the rich menu list API.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RichMenuResponse {
    pub rich_menu_id: String,
    pub name: String,
    pub size: RichMenuSize,
    pub chat_bar_text: String,
    pub selected: bool,
    pub areas: Vec<RichMenuArea>,
}

// ─── API Operations ────────────────────────────────────────────────────────

/// Create a rich menu and return its ID.
pub async fn create_rich_menu(
    http: &reqwest::Client,
    token: &str,
    params: &CreateRichMenuParams,
) -> Result<String> {
    let resp = http
        .post(format!("{LINE_API_BASE}/v2/bot/richmenu"))
        .bearer_auth(token)
        .json(params)
        .send()
        .await
        .context("failed to create rich menu")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("create rich menu failed (HTTP {status}): {body}");
    }

    let body: serde_json::Value = resp.json().await?;
    body["richMenuId"]
        .as_str()
        .map(|s| s.to_string())
        .context("response missing richMenuId")
}

/// Upload an image for a rich menu.
pub async fn upload_rich_menu_image(
    http: &reqwest::Client,
    token: &str,
    rich_menu_id: &str,
    image_data: Vec<u8>,
    content_type: &str,
) -> Result<()> {
    let resp = http
        .post(format!(
            "https://api-data.line.me/v2/bot/richmenu/{rich_menu_id}/content"
        ))
        .bearer_auth(token)
        .header("Content-Type", content_type)
        .body(image_data)
        .send()
        .await
        .context("failed to upload rich menu image")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("upload rich menu image failed (HTTP {status}): {body}");
    }

    Ok(())
}

/// Set a rich menu as the default for all users.
pub async fn set_default_rich_menu(
    http: &reqwest::Client,
    token: &str,
    rich_menu_id: &str,
) -> Result<()> {
    let resp = http
        .post(format!(
            "{LINE_API_BASE}/v2/bot/user/all/richmenu/{rich_menu_id}"
        ))
        .bearer_auth(token)
        .send()
        .await
        .context("failed to set default rich menu")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("set default rich menu failed (HTTP {status}): {body}");
    }

    Ok(())
}

/// Cancel (unset) the default rich menu.
pub async fn cancel_default_rich_menu(
    http: &reqwest::Client,
    token: &str,
) -> Result<()> {
    let resp = http
        .delete(format!("{LINE_API_BASE}/v2/bot/user/all/richmenu"))
        .bearer_auth(token)
        .send()
        .await
        .context("failed to cancel default rich menu")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("cancel default rich menu failed (HTTP {status}): {body}");
    }

    Ok(())
}

/// Delete a rich menu.
pub async fn delete_rich_menu(
    http: &reqwest::Client,
    token: &str,
    rich_menu_id: &str,
) -> Result<()> {
    let resp = http
        .delete(format!(
            "{LINE_API_BASE}/v2/bot/richmenu/{rich_menu_id}"
        ))
        .bearer_auth(token)
        .send()
        .await
        .context("failed to delete rich menu")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("delete rich menu failed (HTTP {status}): {body}");
    }

    Ok(())
}

/// List all rich menus.
pub async fn list_rich_menus(
    http: &reqwest::Client,
    token: &str,
) -> Result<Vec<RichMenuResponse>> {
    let resp = http
        .get(format!("{LINE_API_BASE}/v2/bot/richmenu/list"))
        .bearer_auth(token)
        .send()
        .await
        .context("failed to list rich menus")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("list rich menus failed (HTTP {status}): {body}");
    }

    let body: serde_json::Value = resp.json().await?;
    let menus: Vec<RichMenuResponse> = serde_json::from_value(
        body.get("richmenus").cloned().unwrap_or_default(),
    )
    .unwrap_or_default();

    Ok(menus)
}

/// Link a rich menu to specific users (batched by 500).
pub async fn link_rich_menu_to_users(
    http: &reqwest::Client,
    token: &str,
    user_ids: &[String],
    rich_menu_id: &str,
) -> Result<()> {
    for batch in user_ids.chunks(RICH_MENU_BATCH_LIMIT) {
        let resp = http
            .post(format!(
                "{LINE_API_BASE}/v2/bot/richmenu/bulk/link"
            ))
            .bearer_auth(token)
            .json(&json!({
                "richMenuId": rich_menu_id,
                "userIds": batch,
            }))
            .send()
            .await
            .context("failed to bulk link rich menu")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("bulk link rich menu failed (HTTP {status}): {body}");
        }
    }

    Ok(())
}

/// Unlink rich menus from specific users (batched by 500).
pub async fn unlink_rich_menu_from_users(
    http: &reqwest::Client,
    token: &str,
    user_ids: &[String],
) -> Result<()> {
    for batch in user_ids.chunks(RICH_MENU_BATCH_LIMIT) {
        let resp = http
            .post(format!(
                "{LINE_API_BASE}/v2/bot/richmenu/bulk/unlink"
            ))
            .bearer_auth(token)
            .json(&json!({
                "userIds": batch,
            }))
            .send()
            .await
            .context("failed to bulk unlink rich menu")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("bulk unlink rich menu failed (HTTP {status}): {body}");
        }
    }

    Ok(())
}

// ─── Layout Helpers ────────────────────────────────────────────────────────

/// Create a 2×3 grid layout of equal-sized tap areas.
///
/// `height` should be the menu height (1686 or 843).
/// `actions` must have exactly 6 elements (left-to-right, top-to-bottom).
pub fn create_grid_layout(height: u32, actions: [serde_json::Value; 6]) -> Vec<RichMenuArea> {
    let cell_width = 2500 / 3;
    let cell_height = height / 2;

    actions
        .into_iter()
        .enumerate()
        .map(|(i, action)| {
            let col = (i % 3) as u32;
            let row = (i / 3) as u32;
            RichMenuArea {
                bounds: RichMenuBounds {
                    x: col * cell_width,
                    y: row * cell_height,
                    width: cell_width,
                    height: cell_height,
                },
                action,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grid_layout_produces_6_areas() {
        let actions = [
            json!({"type": "message", "text": "1"}),
            json!({"type": "message", "text": "2"}),
            json!({"type": "message", "text": "3"}),
            json!({"type": "message", "text": "4"}),
            json!({"type": "message", "text": "5"}),
            json!({"type": "message", "text": "6"}),
        ];
        let areas = create_grid_layout(1686, actions);
        assert_eq!(areas.len(), 6);

        // First area: top-left
        assert_eq!(areas[0].bounds.x, 0);
        assert_eq!(areas[0].bounds.y, 0);

        // Last area: bottom-right
        assert_eq!(areas[5].bounds.x, 2 * (2500 / 3));
        assert_eq!(areas[5].bounds.y, 1686 / 2);
    }

    #[test]
    fn menu_sizes() {
        let full = RichMenuSize::full();
        assert_eq!(full.width, 2500);
        assert_eq!(full.height, 1686);

        let half = RichMenuSize::half();
        assert_eq!(half.width, 2500);
        assert_eq!(half.height, 843);
    }
}
