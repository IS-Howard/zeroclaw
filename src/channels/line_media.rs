//! LINE media download with streaming size enforcement and content-type detection.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

/// Result of downloading a LINE media attachment.
#[derive(Debug)]
pub struct MediaDownload {
    /// Path to the downloaded temp file.
    pub path: PathBuf,
    /// Detected MIME type (e.g. `"image/jpeg"`).
    pub content_type: String,
    /// Total bytes downloaded.
    pub size: usize,
}

/// Download media content from the LINE API, enforcing `max_bytes` size limit.
///
/// The file is saved to a random temp path (never derived from `message_id`).
/// Content type is detected via magic bytes, not the Content-Type header.
pub async fn download_line_media(
    http: &reqwest::Client,
    channel_access_token: &str,
    message_id: &str,
    max_bytes: usize,
) -> Result<MediaDownload> {
    let url = format!(
        "https://api-data.line.me/v2/bot/message/{message_id}/content"
    );

    let resp = http
        .get(&url)
        .bearer_auth(channel_access_token)
        .send()
        .await
        .context("LINE media download request failed")?;

    if !resp.status().is_success() {
        bail!(
            "LINE media download returned HTTP {}: {}",
            resp.status(),
            message_id
        );
    }

    // Stream to temp file with size enforcement
    let tmp = tempfile::Builder::new()
        .prefix("zeroclaw-line-")
        .tempfile()
        .context("failed to create temp file for LINE media")?;

    let tmp_path = tmp.into_temp_path().to_path_buf();
    // Keep the file (don't delete on drop) — caller manages lifecycle.
    let _ = tmp_path.clone();

    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .context("failed to open temp file for writing")?;

    let mut stream = resp.bytes_stream();
    let mut total: usize = 0;
    let mut header_buf = Vec::with_capacity(16);

    use tokio_stream::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("error reading LINE media stream")?;
        total += chunk.len();
        if total > max_bytes {
            // Clean up partial file
            let _ = tokio::fs::remove_file(&tmp_path).await;
            bail!(
                "LINE media exceeds size limit ({total} > {max_bytes} bytes)"
            );
        }
        // Capture first bytes for magic-byte detection
        if header_buf.len() < 16 {
            let need = 16 - header_buf.len();
            header_buf.extend_from_slice(&chunk[..chunk.len().min(need)]);
        }
        file.write_all(&chunk).await.context("failed to write LINE media chunk")?;
    }
    file.flush().await?;

    if total == 0 {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        bail!("LINE media download returned empty body");
    }

    let content_type = detect_content_type(&header_buf);

    // Rename to have correct extension
    let ext = mime_to_extension(&content_type);
    let final_path = tmp_path.with_extension(ext);
    tokio::fs::rename(&tmp_path, &final_path)
        .await
        .unwrap_or_else(|_| {
            // If rename fails (cross-device), just use original path
        });
    let path = if final_path.exists() {
        final_path
    } else {
        tmp_path
    };

    Ok(MediaDownload {
        path,
        content_type,
        size: total,
    })
}

/// Detect content type from file header magic bytes.
fn detect_content_type(header: &[u8]) -> String {
    if header.len() >= 2 && header[0] == 0xFF && header[1] == 0xD8 {
        return "image/jpeg".to_string();
    }
    if header.len() >= 4 && header[..4] == [0x89, 0x50, 0x4E, 0x47] {
        return "image/png".to_string();
    }
    if header.len() >= 3 && header[..3] == [0x47, 0x49, 0x46] {
        return "image/gif".to_string();
    }
    if header.len() >= 12
        && header[..4] == [0x52, 0x49, 0x46, 0x46]
        && header[8..12] == [0x57, 0x45, 0x42, 0x50]
    {
        return "image/webp".to_string();
    }
    if header.len() >= 8 && header[4..8] == [0x66, 0x74, 0x79, 0x70] {
        return "video/mp4".to_string();
    }
    if header.len() >= 4 && header[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return "video/webm".to_string();
    }
    "application/octet-stream".to_string()
}

/// Map MIME type to a file extension.
fn mime_to_extension(content_type: &str) -> &'static str {
    match content_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "audio/mp4" | "audio/aac" => "m4a",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_jpeg() {
        assert_eq!(detect_content_type(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
    }

    #[test]
    fn detect_png() {
        assert_eq!(
            detect_content_type(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A]),
            "image/png"
        );
    }

    #[test]
    fn detect_gif() {
        assert_eq!(detect_content_type(&[0x47, 0x49, 0x46, 0x38]), "image/gif");
    }

    #[test]
    fn detect_webp() {
        assert_eq!(
            detect_content_type(&[
                0x52, 0x49, 0x46, 0x46, 0x00, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50
            ]),
            "image/webp"
        );
    }

    #[test]
    fn detect_mp4() {
        assert_eq!(
            detect_content_type(&[0x00, 0x00, 0x00, 0x20, 0x66, 0x74, 0x79, 0x70]),
            "video/mp4"
        );
    }

    #[test]
    fn detect_unknown() {
        assert_eq!(detect_content_type(&[0x00, 0x01]), "application/octet-stream");
    }

    #[test]
    fn detect_empty() {
        assert_eq!(detect_content_type(&[]), "application/octet-stream");
    }

    #[test]
    fn extension_mapping() {
        assert_eq!(mime_to_extension("image/jpeg"), "jpg");
        assert_eq!(mime_to_extension("video/mp4"), "mp4");
        assert_eq!(mime_to_extension("application/octet-stream"), "bin");
    }
}
