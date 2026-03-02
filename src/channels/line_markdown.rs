//! Markdown → LINE Flex Message conversion pipeline.
//!
//! Extracts structured content (tables, code blocks) from markdown text,
//! converts them to LINE Flex Message bubbles, and strips remaining markdown
//! formatting to plain text suitable for LINE's text message type.

use super::line_flex;
use serde_json::Value;

/// Result of processing a markdown message for LINE.
pub struct ProcessedLineMessage {
    /// Plain text with markdown stripped (for text message).
    pub text: String,
    /// Flex messages extracted from structured content.
    pub flex_messages: Vec<Value>,
}

/// A parsed markdown table.
struct MarkdownTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// A parsed fenced code block.
struct CodeBlock {
    language: Option<String>,
    code: String,
}

/// Main processing pipeline: extract structured content, convert to Flex, strip markdown.
pub fn process_line_message(text: &str) -> ProcessedLineMessage {
    let mut flex_messages = Vec::new();

    // 1. Extract and convert markdown tables
    let (tables, text_without_tables) = extract_markdown_tables(text);
    for table in &tables {
        if let Some(bubble) = convert_table_to_flex_bubble(table) {
            flex_messages.push(line_flex::to_flex_message("Table", bubble));
        }
    }

    // 2. Extract and convert code blocks
    let (code_blocks, text_without_code) = extract_code_blocks(&text_without_tables);
    for block in &code_blocks {
        let bubble = line_flex::create_code_bubble(
            &block.code,
            block.language.as_deref(),
        );
        flex_messages.push(line_flex::to_flex_message("Code", bubble));
    }

    // 3. Strip remaining markdown formatting
    let plain = strip_markdown(&text_without_code);

    ProcessedLineMessage {
        text: plain.trim().to_string(),
        flex_messages,
    }
}

/// Check if text contains markdown that would benefit from conversion.
pub fn has_markdown_to_convert(text: &str) -> bool {
    // Tables
    if text.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.matches('|').count() >= 3
    }) {
        return true;
    }
    // Code blocks
    if text.contains("```") {
        return true;
    }
    false
}

// ─── Table Extraction ──────────────────────────────────────────────────────

/// Extract pipe-delimited markdown tables from text.
/// Returns the tables and the text with tables removed.
fn extract_markdown_tables(text: &str) -> (Vec<MarkdownTable>, String) {
    let mut tables = Vec::new();
    let mut remaining = String::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Check if this line starts a table (has | delimiters)
        if is_table_row(line) && i + 1 < lines.len() && is_separator_row(lines[i + 1].trim()) {
            // Parse header row
            let headers = parse_table_row(line);

            // Skip separator row
            i += 2;

            // Parse data rows
            let mut rows = Vec::new();
            while i < lines.len() && is_table_row(lines[i].trim()) {
                rows.push(parse_table_row(lines[i].trim()));
                i += 1;
            }

            if !headers.is_empty() && !rows.is_empty() {
                tables.push(MarkdownTable { headers, rows });
            }
            continue;
        }

        remaining.push_str(lines[i]);
        remaining.push('\n');
        i += 1;
    }

    (tables, remaining)
}

fn is_table_row(line: &str) -> bool {
    line.starts_with('|') && line.ends_with('|') && line.matches('|').count() >= 3
}

fn is_separator_row(line: &str) -> bool {
    line.starts_with('|')
        && line
            .chars()
            .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
}

fn parse_table_row(line: &str) -> Vec<String> {
    line.trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// Convert a markdown table to a Flex receipt or list card.
fn convert_table_to_flex_bubble(table: &MarkdownTable) -> Option<Value> {
    if table.headers.is_empty() || table.rows.is_empty() {
        return None;
    }

    if table.headers.len() == 2 {
        // 2-column table → receipt-style card
        let rows: Vec<(String, String)> = table
            .rows
            .iter()
            .take(20)
            .map(|row| {
                let col0 = row.first().cloned().unwrap_or_default();
                let col1 = row.get(1).cloned().unwrap_or_default();
                (col0, col1)
            })
            .collect();
        let title = format!("{} / {}", table.headers[0], table.headers[1]);
        Some(line_flex::create_receipt_card(&title, &rows, None))
    } else {
        // 3+ column table → list card with concatenated values
        let items: Vec<(String, String)> = table
            .rows
            .iter()
            .take(10)
            .map(|row| {
                let key = row.first().cloned().unwrap_or_default();
                let rest: Vec<&str> = row.iter().skip(1).map(|s| s.as_str()).collect();
                (key, rest.join(" | "))
            })
            .collect();
        let title = table.headers.join(" | ");
        Some(line_flex::create_list_card(&title, &items))
    }
}

// ─── Code Block Extraction ─────────────────────────────────────────────────

/// Extract fenced code blocks (``` ... ```) from text.
fn extract_code_blocks(text: &str) -> (Vec<CodeBlock>, String) {
    let mut blocks = Vec::new();
    let mut remaining = String::new();
    #[allow(unused)]
    let chars = text.chars().peekable();
    let mut in_block = false;
    let mut current_lang: Option<String> = None;
    let mut current_code = String::new();

    let mut line_buf = String::new();

    for ch in text.chars() {
        if ch == '\n' {
            let line = line_buf.trim_end();
            if !in_block && line.starts_with("```") {
                in_block = true;
                let lang = line[3..].trim().to_string();
                current_lang = if lang.is_empty() { None } else { Some(lang) };
                current_code.clear();
            } else if in_block && line.starts_with("```") {
                in_block = false;
                if !current_code.trim().is_empty() {
                    blocks.push(CodeBlock {
                        language: current_lang.take(),
                        code: current_code.trim().to_string(),
                    });
                }
                current_code.clear();
            } else if in_block {
                if !current_code.is_empty() {
                    current_code.push('\n');
                }
                current_code.push_str(&line_buf);
            } else {
                remaining.push_str(&line_buf);
                remaining.push('\n');
            }
            line_buf.clear();
        } else {
            line_buf.push(ch);
        }
    }

    // Handle last line without trailing newline
    if !line_buf.is_empty() {
        if in_block {
            // Unclosed code block — treat as remaining text
            remaining.push_str("```");
            if let Some(ref lang) = current_lang {
                remaining.push_str(lang);
            }
            remaining.push('\n');
            remaining.push_str(&current_code);
            remaining.push_str(&line_buf);
        } else {
            remaining.push_str(&line_buf);
        }
    }

    // Suppress unused variable warning
    drop(chars);

    (blocks, remaining)
}

// ─── Markdown Stripping ────────────────────────────────────────────────────

/// Strip markdown formatting to produce plain text suitable for LINE.
pub fn strip_markdown(text: &str) -> String {
    let mut result = text.to_string();

    // Bold: **text** or __text__
    result = regex_replace(&result, r"\*\*(.+?)\*\*", "$1");
    result = regex_replace(&result, r"__(.+?)__", "$1");

    // Italic: *text* or _text_ (careful not to match already-stripped bold)
    result = regex_replace(&result, r"(?<!\*)\*([^*]+?)\*(?!\*)", "$1");
    result = regex_replace(&result, r"(?<!_)_([^_]+?)_(?!_)", "$1");

    // Strikethrough: ~~text~~
    result = regex_replace(&result, r"~~(.+?)~~", "$1");

    // Inline code: `code`
    result = regex_replace(&result, r"`([^`]+?)`", "$1");

    // Headers: # Title
    result = regex_replace(&result, r"(?m)^#{1,6}\s+(.+)$", "$1");

    // Blockquotes: > text
    result = regex_replace(&result, r"(?m)^>\s?(.*)$", "$1");

    // Links: [text](url) → text
    result = regex_replace(&result, r"\[([^\]]+)\]\([^)]+\)", "$1");

    // Images: ![alt](url) → alt
    result = regex_replace(&result, r"!\[([^\]]*)\]\([^)]+\)", "$1");

    // Horizontal rules
    result = regex_replace(&result, r"(?m)^[-*_]{3,}$", "");

    // Unordered list markers: - item or * item
    result = regex_replace(&result, r"(?m)^[\s]*[-*+]\s+", "");

    // Ordered list markers: 1. item
    result = regex_replace(&result, r"(?m)^[\s]*\d+\.\s+", "");

    // Collapse multiple blank lines
    result = regex_replace(&result, r"\n{3,}", "\n\n");

    result
}

/// Helper: apply a regex replacement.
fn regex_replace(text: &str, pattern: &str, replacement: &str) -> String {
    regex::Regex::new(pattern)
        .map(|re| re.replace_all(text, replacement).to_string())
        .unwrap_or_else(|_| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_plain_text_unchanged() {
        let result = process_line_message("Hello world");
        assert_eq!(result.text, "Hello world");
        assert!(result.flex_messages.is_empty());
    }

    #[test]
    fn extract_two_column_table() {
        let text = "Before\n| Key | Value |\n|-----|-------|\n| A | 1 |\n| B | 2 |\nAfter";
        let result = process_line_message(text);
        assert_eq!(result.flex_messages.len(), 1);
        assert!(result.text.contains("Before"));
        assert!(result.text.contains("After"));
        assert!(!result.text.contains("Key"));
    }

    #[test]
    fn extract_code_block() {
        let text = "Here is code:\n```rust\nfn main() {}\n```\nDone.";
        let result = process_line_message(text);
        assert_eq!(result.flex_messages.len(), 1);
        assert!(result.text.contains("Here is code:"));
        assert!(result.text.contains("Done."));
        assert!(!result.text.contains("fn main"));
    }

    #[test]
    fn strip_bold() {
        assert_eq!(strip_markdown("**bold** text"), "bold text");
    }

    #[test]
    fn strip_italic() {
        assert_eq!(strip_markdown("*italic* text"), "italic text");
    }

    #[test]
    fn strip_strikethrough() {
        assert_eq!(strip_markdown("~~deleted~~"), "deleted");
    }

    #[test]
    fn strip_inline_code() {
        assert_eq!(strip_markdown("`code` here"), "code here");
    }

    #[test]
    fn strip_headers() {
        assert_eq!(strip_markdown("## Title").trim(), "Title");
    }

    #[test]
    fn strip_links() {
        assert_eq!(
            strip_markdown("[click here](https://example.com)"),
            "click here"
        );
    }

    #[test]
    fn strip_blockquotes() {
        assert_eq!(strip_markdown("> quoted text").trim(), "quoted text");
    }

    #[test]
    fn has_markdown_detects_table() {
        assert!(has_markdown_to_convert("| A | B |\n|---|---|\n| 1 | 2 |"));
    }

    #[test]
    fn has_markdown_detects_code() {
        assert!(has_markdown_to_convert("```\ncode\n```"));
    }

    #[test]
    fn has_markdown_plain_text_false() {
        assert!(!has_markdown_to_convert("Just plain text here"));
    }

    #[test]
    fn three_column_table_becomes_list_card() {
        let text = "| A | B | C |\n|---|---|---|\n| 1 | 2 | 3 |";
        let result = process_line_message(text);
        assert_eq!(result.flex_messages.len(), 1);
    }
}
