//! LINE Template Message builders.
//!
//! Template messages are interactive messages with predefined layouts:
//! confirm dialogs, button menus, carousels, and image carousels.

use serde_json::{json, Value};

// ─── Action Types ──────────────────────────────────────────────────────────

/// LINE template action.
#[derive(Debug, Clone)]
pub enum TemplateAction {
    /// Opens a URL in the user's browser.
    Uri { label: String, uri: String },
    /// Sends postback data to the webhook.
    Postback { label: String, data: String },
    /// Sends a text message as the user.
    Message { label: String, text: String },
}

impl TemplateAction {
    fn to_json(&self) -> Value {
        match self {
            Self::Uri { label, uri } => json!({
                "type": "uri",
                "label": truncate_label(label),
                "uri": uri,
            }),
            Self::Postback { label, data } => json!({
                "type": "postback",
                "label": truncate_label(label),
                "data": data,
            }),
            Self::Message { label, text } => json!({
                "type": "message",
                "label": truncate_label(label),
                "text": text,
            }),
        }
    }

    /// Parse an action from `"Label|value"` format.
    /// Auto-detects: URLs → Uri, `key=value` → Postback, otherwise → Message.
    pub fn parse(input: &str) -> Self {
        let (label, value) = if let Some((l, v)) = input.split_once('|') {
            (l.trim().to_string(), v.trim().to_string())
        } else {
            (input.trim().to_string(), input.trim().to_string())
        };

        if value.starts_with("http://") || value.starts_with("https://") {
            Self::Uri { label, uri: value }
        } else if value.contains('=') {
            Self::Postback { label, data: value }
        } else {
            Self::Message { label, text: value }
        }
    }
}

// ─── Template Builders ─────────────────────────────────────────────────────

/// Create a confirm template (yes/no dialog).
///
/// LINE limit: text ≤240 chars, exactly 2 actions.
pub fn create_confirm_template(
    text: &str,
    confirm_action: TemplateAction,
    cancel_action: TemplateAction,
    alt_text: Option<&str>,
) -> Value {
    json!({
        "type": "template",
        "altText": alt_text.unwrap_or(text).chars().take(400).collect::<String>(),
        "template": {
            "type": "confirm",
            "text": text.chars().take(240).collect::<String>(),
            "actions": [
                confirm_action.to_json(),
                cancel_action.to_json(),
            ],
        },
    })
}

/// Create a buttons template (title + text + up to 4 action buttons).
///
/// LINE limits: title ≤40 chars, text ≤160 chars (with image) or ≤60 chars, max 4 actions.
pub fn create_button_template(
    title: &str,
    text: &str,
    actions: &[TemplateAction],
    thumbnail_url: Option<&str>,
) -> Value {
    let actions_json: Vec<Value> = actions.iter().take(4).map(|a| a.to_json()).collect();
    let alt = format!("{title}: {text}");

    let mut template = json!({
        "type": "buttons",
        "title": title.chars().take(40).collect::<String>(),
        "text": text.chars().take(160).collect::<String>(),
        "actions": actions_json,
    });

    if let Some(url) = thumbnail_url {
        template["thumbnailImageUrl"] = json!(url);
    }

    json!({
        "type": "template",
        "altText": alt.chars().take(400).collect::<String>(),
        "template": template,
    })
}

/// A single column in a carousel template.
pub struct CarouselColumn {
    pub title: String,
    pub text: String,
    pub actions: Vec<TemplateAction>,
    pub thumbnail_url: Option<String>,
}

/// Create a carousel template (up to 10 columns, max 3 actions each).
pub fn create_carousel(columns: &[CarouselColumn], alt_text: Option<&str>) -> Value {
    let cols: Vec<Value> = columns
        .iter()
        .take(10)
        .map(|col| {
            let actions: Vec<Value> = col.actions.iter().take(3).map(|a| a.to_json()).collect();
            let mut column = json!({
                "title": col.title.chars().take(40).collect::<String>(),
                "text": col.text.chars().take(120).collect::<String>(),
                "actions": actions,
            });
            if let Some(ref url) = col.thumbnail_url {
                column["thumbnailImageUrl"] = json!(url);
            }
            column
        })
        .collect();

    json!({
        "type": "template",
        "altText": alt_text.unwrap_or("Carousel").chars().take(400).collect::<String>(),
        "template": {
            "type": "carousel",
            "columns": cols,
        },
    })
}

/// A column in an image carousel.
pub struct ImageCarouselColumn {
    pub image_url: String,
    pub action: TemplateAction,
}

/// Create an image carousel template (up to 10 image columns).
pub fn create_image_carousel(columns: &[ImageCarouselColumn], alt_text: Option<&str>) -> Value {
    let cols: Vec<Value> = columns
        .iter()
        .take(10)
        .map(|col| {
            json!({
                "imageUrl": col.image_url,
                "action": col.action.to_json(),
            })
        })
        .collect();

    json!({
        "type": "template",
        "altText": alt_text.unwrap_or("Image carousel").chars().take(400).collect::<String>(),
        "template": {
            "type": "image_carousel",
            "columns": cols,
        },
    })
}

// ─── Quick Replies ─────────────────────────────────────────────────────────

/// Create quick reply items from text labels.
///
/// Each label becomes a text-sending quick reply button.
pub fn create_quick_reply_items(labels: &[String]) -> Value {
    let items: Vec<Value> = labels
        .iter()
        .take(13) // LINE limit: max 13 quick reply items
        .map(|label| {
            json!({
                "type": "action",
                "action": {
                    "type": "message",
                    "label": truncate_label(label),
                    "text": label,
                },
            })
        })
        .collect();

    json!({ "items": items })
}

/// Attach quick reply items to an existing message object.
pub fn attach_quick_replies(message: &mut Value, labels: &[String]) {
    if !labels.is_empty() {
        message["quickReply"] = create_quick_reply_items(labels);
    }
}

// ─── Utilities ─────────────────────────────────────────────────────────────

/// Truncate a button label to LINE's 20-char limit.
fn truncate_label(s: &str) -> String {
    if s.chars().count() <= 20 {
        s.to_string()
    } else {
        s.chars().take(17).collect::<String>() + "..."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_template_structure() {
        let msg = create_confirm_template(
            "Delete?",
            TemplateAction::Message {
                label: "Yes".into(),
                text: "yes".into(),
            },
            TemplateAction::Message {
                label: "No".into(),
                text: "no".into(),
            },
            None,
        );
        assert_eq!(msg["type"], "template");
        assert_eq!(msg["template"]["type"], "confirm");
        assert_eq!(msg["template"]["actions"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn button_template_max_4_actions() {
        let actions: Vec<TemplateAction> = (0..6)
            .map(|i| TemplateAction::Message {
                label: format!("Btn{i}"),
                text: format!("msg{i}"),
            })
            .collect();
        let msg = create_button_template("Title", "Text", &actions, None);
        assert_eq!(msg["template"]["actions"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn carousel_max_10_columns() {
        let columns: Vec<CarouselColumn> = (0..15)
            .map(|i| CarouselColumn {
                title: format!("Col {i}"),
                text: format!("Text {i}"),
                actions: vec![TemplateAction::Message {
                    label: "Go".into(),
                    text: "go".into(),
                }],
                thumbnail_url: None,
            })
            .collect();
        let msg = create_carousel(&columns, None);
        assert_eq!(
            msg["template"]["columns"].as_array().unwrap().len(),
            10
        );
    }

    #[test]
    fn quick_reply_max_13() {
        let labels: Vec<String> = (0..20).map(|i| format!("Option {i}")).collect();
        let qr = create_quick_reply_items(&labels);
        assert_eq!(qr["items"].as_array().unwrap().len(), 13);
    }

    #[test]
    fn parse_action_uri() {
        let action = TemplateAction::parse("Visit|https://example.com");
        assert!(matches!(action, TemplateAction::Uri { .. }));
    }

    #[test]
    fn parse_action_postback() {
        let action = TemplateAction::parse("Select|action=buy&item=1");
        assert!(matches!(action, TemplateAction::Postback { .. }));
    }

    #[test]
    fn parse_action_message() {
        let action = TemplateAction::parse("Hello|hello");
        assert!(matches!(action, TemplateAction::Message { .. }));
    }

    #[test]
    fn label_truncation() {
        assert_eq!(truncate_label("short"), "short");
        assert_eq!(
            truncate_label("this is a very long label text"),
            "this is a very lo..."
        );
    }
}
