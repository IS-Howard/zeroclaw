//! LINE Flex Message builders.
//!
//! Flex Messages are rich, interactive messages rendered as JSON-defined layouts.
//! This module provides builder functions that produce `serde_json::Value` payloads
//! conforming to the LINE Flex Message specification.

use serde_json::{json, Value};

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Wrap a Flex bubble in a complete Flex message object.
pub fn to_flex_message(alt_text: &str, contents: Value) -> Value {
    json!({
        "type": "flex",
        "altText": truncate(alt_text, 400),
        "contents": contents,
    })
}

/// Create a Flex carousel container wrapping multiple bubbles.
pub fn create_carousel(bubbles: Vec<Value>) -> Value {
    json!({
        "type": "carousel",
        "contents": bubbles,
    })
}

// ─── Card Builders ─────────────────────────────────────────────────────────

/// Info card: title + body text.
pub fn create_info_card(title: &str, body: &str) -> Value {
    json!({
        "type": "bubble",
        "body": {
            "type": "box",
            "layout": "vertical",
            "contents": [
                {
                    "type": "text",
                    "text": truncate(title, 200),
                    "weight": "bold",
                    "size": "lg",
                    "wrap": true,
                },
                {
                    "type": "text",
                    "text": truncate(body, 2000),
                    "size": "sm",
                    "color": "#666666",
                    "wrap": true,
                    "margin": "md",
                },
            ],
        },
    })
}

/// Image card: hero image + optional title + optional body.
pub fn create_image_card(image_url: &str, title: Option<&str>, body: Option<&str>) -> Value {
    let mut contents: Vec<Value> = Vec::new();

    if let Some(t) = title {
        contents.push(json!({
            "type": "text",
            "text": truncate(t, 200),
            "weight": "bold",
            "size": "lg",
            "wrap": true,
        }));
    }
    if let Some(b) = body {
        contents.push(json!({
            "type": "text",
            "text": truncate(b, 2000),
            "size": "sm",
            "color": "#666666",
            "wrap": true,
            "margin": "md",
        }));
    }

    let mut bubble = json!({
        "type": "bubble",
        "hero": {
            "type": "image",
            "url": image_url,
            "size": "full",
            "aspectRatio": "20:13",
            "aspectMode": "cover",
        },
    });

    if !contents.is_empty() {
        bubble["body"] = json!({
            "type": "box",
            "layout": "vertical",
            "contents": contents,
        });
    }

    bubble
}

/// Action card: title + body + action buttons.
pub fn create_action_card(title: &str, body: &str, actions: &[ActionButton]) -> Value {
    let footer_contents: Vec<Value> = actions
        .iter()
        .take(4) // LINE limit: max 4 actions
        .map(|a| a.to_flex_button())
        .collect();

    json!({
        "type": "bubble",
        "body": {
            "type": "box",
            "layout": "vertical",
            "contents": [
                {
                    "type": "text",
                    "text": truncate(title, 200),
                    "weight": "bold",
                    "size": "lg",
                    "wrap": true,
                },
                {
                    "type": "text",
                    "text": truncate(body, 2000),
                    "size": "sm",
                    "color": "#666666",
                    "wrap": true,
                    "margin": "md",
                },
            ],
        },
        "footer": {
            "type": "box",
            "layout": "vertical",
            "spacing": "sm",
            "contents": footer_contents,
        },
    })
}

/// List card: title + list of label-value pairs.
pub fn create_list_card(title: &str, items: &[(String, String)]) -> Value {
    let mut rows: Vec<Value> = Vec::new();
    for (label, value) in items.iter().take(20) {
        rows.push(json!({
            "type": "box",
            "layout": "horizontal",
            "contents": [
                {
                    "type": "text",
                    "text": truncate(label, 100),
                    "size": "sm",
                    "color": "#555555",
                    "flex": 0,
                },
                {
                    "type": "text",
                    "text": truncate(value, 200),
                    "size": "sm",
                    "color": "#111111",
                    "align": "end",
                },
            ],
        }));
    }

    json!({
        "type": "bubble",
        "body": {
            "type": "box",
            "layout": "vertical",
            "contents": [
                {
                    "type": "text",
                    "text": truncate(title, 200),
                    "weight": "bold",
                    "size": "lg",
                },
                { "type": "separator", "margin": "md" },
                {
                    "type": "box",
                    "layout": "vertical",
                    "margin": "md",
                    "spacing": "sm",
                    "contents": rows,
                },
            ],
        },
    })
}

/// Receipt card: title + key-value rows + optional total row.
pub fn create_receipt_card(
    title: &str,
    rows: &[(String, String)],
    total: Option<(&str, &str)>,
) -> Value {
    let mut body_contents: Vec<Value> = vec![json!({
        "type": "text",
        "text": truncate(title, 200),
        "weight": "bold",
        "size": "lg",
    })];

    body_contents.push(json!({ "type": "separator", "margin": "md" }));

    let row_items: Vec<Value> = rows
        .iter()
        .take(20)
        .map(|(k, v)| {
            json!({
                "type": "box",
                "layout": "horizontal",
                "contents": [
                    { "type": "text", "text": truncate(k, 100), "size": "sm", "color": "#555555", "flex": 0 },
                    { "type": "text", "text": truncate(v, 200), "size": "sm", "color": "#111111", "align": "end" },
                ],
            })
        })
        .collect();

    body_contents.push(json!({
        "type": "box",
        "layout": "vertical",
        "margin": "md",
        "spacing": "sm",
        "contents": row_items,
    }));

    if let Some((label, value)) = total {
        body_contents.push(json!({ "type": "separator", "margin": "md" }));
        body_contents.push(json!({
            "type": "box",
            "layout": "horizontal",
            "margin": "md",
            "contents": [
                { "type": "text", "text": truncate(label, 100), "size": "md", "weight": "bold", "color": "#555555" },
                { "type": "text", "text": truncate(value, 200), "size": "md", "weight": "bold", "align": "end" },
            ],
        }));
    }

    json!({
        "type": "bubble",
        "body": {
            "type": "box",
            "layout": "vertical",
            "contents": body_contents,
        },
    })
}

/// Code bubble: displays a code block with optional language label.
pub fn create_code_bubble(code: &str, language: Option<&str>) -> Value {
    let truncated_code = truncate(code, 2000);
    let mut header_contents: Vec<Value> = Vec::new();

    if let Some(lang) = language {
        header_contents.push(json!({
            "type": "text",
            "text": lang,
            "size": "xs",
            "color": "#AAAAAA",
            "weight": "bold",
        }));
    }

    let mut bubble = json!({
        "type": "bubble",
        "body": {
            "type": "box",
            "layout": "vertical",
            "backgroundColor": "#1E1E1E",
            "paddingAll": "lg",
            "contents": [
                {
                    "type": "text",
                    "text": truncated_code,
                    "size": "xs",
                    "color": "#CCCCCC",
                    "wrap": true,
                },
            ],
        },
    });

    if !header_contents.is_empty() {
        bubble["header"] = json!({
            "type": "box",
            "layout": "vertical",
            "backgroundColor": "#1E1E1E",
            "paddingBottom": "none",
            "contents": header_contents,
        });
    }

    bubble
}

// ─── Action Button ─────────────────────────────────────────────────────────

/// An action button for Flex Message footers.
pub struct ActionButton {
    pub label: String,
    pub action: ButtonAction,
}

/// Button action type.
pub enum ButtonAction {
    /// Opens a URL.
    Uri(String),
    /// Sends postback data.
    Postback(String),
    /// Sends a text message.
    Message(String),
}

impl ActionButton {
    fn to_flex_button(&self) -> Value {
        let label = truncate(&self.label, 20);
        let action = match &self.action {
            ButtonAction::Uri(uri) => json!({
                "type": "uri",
                "label": label,
                "uri": uri,
            }),
            ButtonAction::Postback(data) => json!({
                "type": "postback",
                "label": label,
                "data": data,
            }),
            ButtonAction::Message(text) => json!({
                "type": "message",
                "label": label,
                "text": text,
            }),
        };

        json!({
            "type": "button",
            "style": "primary",
            "action": action,
        })
    }
}

// ─── Utilities ─────────────────────────────────────────────────────────────

/// Truncate a string to `max_chars` characters, appending "..." if truncated.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flex_message_structure() {
        let bubble = create_info_card("Test", "Body text");
        let msg = to_flex_message("alt", bubble);
        assert_eq!(msg["type"], "flex");
        assert_eq!(msg["altText"], "alt");
        assert_eq!(msg["contents"]["type"], "bubble");
    }

    #[test]
    fn carousel_wraps_bubbles() {
        let b1 = create_info_card("A", "a");
        let b2 = create_info_card("B", "b");
        let carousel = create_carousel(vec![b1, b2]);
        assert_eq!(carousel["type"], "carousel");
        assert_eq!(carousel["contents"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn code_bubble_has_dark_background() {
        let bubble = create_code_bubble("fn main() {}", Some("rust"));
        assert_eq!(bubble["body"]["backgroundColor"], "#1E1E1E");
        assert_eq!(bubble["header"]["contents"][0]["text"], "rust");
    }

    #[test]
    fn receipt_card_with_total() {
        let rows = vec![
            ("Item A".into(), "$10".into()),
            ("Item B".into(), "$20".into()),
        ];
        let card = create_receipt_card("Receipt", &rows, Some(("Total", "$30")));
        let body = card["body"]["contents"].as_array().unwrap();
        // title + separator + rows box + separator + total = 5
        assert_eq!(body.len(), 5);
    }

    #[test]
    fn action_card_limits_buttons() {
        let actions: Vec<ActionButton> = (0..6)
            .map(|i| ActionButton {
                label: format!("Btn {i}"),
                action: ButtonAction::Message(format!("msg {i}")),
            })
            .collect();
        let card = create_action_card("Title", "Body", &actions);
        let footer = card["footer"]["contents"].as_array().unwrap();
        assert_eq!(footer.len(), 4); // max 4
    }

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        assert_eq!(truncate("hello world", 8), "hello...");
    }

    #[test]
    fn image_card_without_text() {
        let card = create_image_card("https://example.com/img.jpg", None, None);
        assert_eq!(card["hero"]["type"], "image");
        assert!(card.get("body").is_none());
    }
}
