/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/discord/client.rs
 * Responsibility: Outbound Discord messaging helpers and payload chunking.
 */

use once_cell::sync::Lazy;
use serenity::all::CreateAttachment;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

static HTTP_CHANNEL_CLIENT: Lazy<Arc<RwLock<Option<(String, Arc<serenity::http::Http>)>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

fn split_message_chunks(content: &str, max_length: usize) -> Vec<String> {
    if max_length == 0 || content.is_empty() {
        return Vec::new();
    }

    let mut remaining = content;
    let mut chunks = Vec::new();

    while !remaining.is_empty() {
        let char_count = remaining.chars().count();
        if char_count <= max_length {
            chunks.push(remaining.to_string());
            break;
        }

        let mut boundary = remaining.len();
        for (count, (idx, _)) in remaining.char_indices().enumerate() {
            if count == max_length {
                boundary = idx;
                break;
            }
        }

        let candidate = &remaining[..boundary];
        let split_at = candidate
            .rfind('\n')
            .filter(|idx| *idx >= boundary / 3)
            .map(|idx| idx + 1)
            .unwrap_or(boundary);

        let chunk = remaining[..split_at].trim_end_matches('\n').to_string();
        if chunk.is_empty() {
            chunks.push(candidate.to_string());
            remaining = &remaining[boundary..];
        } else {
            chunks.push(chunk);
            remaining = remaining[split_at..].trim_start_matches('\n');
        }
    }

    chunks
}

fn split_code_block_chunks(content: &str, language: &str, max_length: usize) -> Vec<String> {
    let fence_overhead = if language.is_empty() {
        "```\n\n```".len()
    } else {
        format!("```{}\n\n```", language).len()
    };

    if max_length <= fence_overhead {
        return Vec::new();
    }

    split_message_chunks(content, max_length - fence_overhead)
}

async fn get_http_client(token: &str) -> Arc<serenity::http::Http> {
    let mut client_lock = HTTP_CHANNEL_CLIENT.write().await;
    let reuse = if let Some((existing_token, _)) = &*client_lock {
        existing_token == token
    } else {
        false
    };

    if !reuse {
        let new_http = Arc::new(serenity::http::Http::new(token));
        *client_lock = Some((token.to_string(), new_http.clone()));
        new_http
    } else {
        client_lock.as_ref().unwrap().1.clone()
    }
}

pub async fn send_bot_message(
    token: &str,
    channel_id: &str,
    content: &str,
) -> anyhow::Result<serenity::model::channel::Message> {
    if token.is_empty() {
        return Err(anyhow::anyhow!("Discord token is empty"));
    }
    if channel_id.is_empty() || channel_id == "0" {
        return Err(anyhow::anyhow!("Invalid channel ID: {}", channel_id));
    }

    let http = get_http_client(token).await;
    let c_id = channel_id
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid channel ID: {}", channel_id))?;

    let max_length = 1900;
    let mut last_msg = None;
    let chunks = split_message_chunks(content, max_length);
    if chunks.len() > 1 {
        println!(
            "✂️ Content length {} exceeds Discord limit, chunking...",
            content.len()
        );
    }

    for chunk in chunks {
        let map = serde_json::json!({ "content": chunk });
        last_msg = Some(http.send_message(c_id.into(), vec![], &map).await?);
        if last_msg.is_some() {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }

    last_msg.ok_or_else(|| anyhow::anyhow!("Failed to send any message chunks"))
}

pub async fn send_code_block_message(
    token: &str,
    channel_id: &str,
    content: &str,
    language: &str,
) -> anyhow::Result<serenity::model::channel::Message> {
    let max_length = 1900;
    let chunks = split_code_block_chunks(content, language, max_length);

    if chunks.is_empty() {
        return Err(anyhow::anyhow!("Failed to build any code block chunks"));
    }

    let mut last_msg = None;
    for chunk in chunks {
        let fenced = if language.is_empty() {
            format!("```\n{}\n```", chunk)
        } else {
            format!("```{}\n{}\n```", language, chunk)
        };
        last_msg = Some(send_bot_message(token, channel_id, &fenced).await?);
    }

    last_msg.ok_or_else(|| anyhow::anyhow!("Failed to send any code block chunks"))
}

pub async fn send_reply_message(
    token: &str,
    channel_id: &str,
    message_id: &str,
    content: &str,
) -> anyhow::Result<serenity::model::channel::Message> {
    if token.is_empty() {
        return Err(anyhow::anyhow!("Discord token is empty"));
    }
    if channel_id.is_empty() || channel_id == "0" {
        return Err(anyhow::anyhow!("Invalid channel ID: {}", channel_id));
    }
    if message_id.is_empty() {
        return Err(anyhow::anyhow!("Invalid message ID"));
    }

    let http = get_http_client(token).await;
    let c_id = channel_id
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid channel ID: {}", channel_id))?;
    let m_id = message_id
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid message ID: {}", message_id))?;

    let map = serde_json::json!({
        "content": content,
        "message_reference": { "message_id": m_id.to_string() },
        "allowed_mentions": { "replied_user": false }
    });
    let msg = http.send_message(c_id.into(), vec![], &map).await?;
    Ok(msg)
}

pub async fn send_embed_message(
    token: &str,
    channel_id: &str,
    title: &str,
    description: &str,
    color: Option<u32>,
) -> anyhow::Result<serenity::model::channel::Message> {
    if token.is_empty() {
        return Err(anyhow::anyhow!("Discord token is empty"));
    }
    if channel_id.is_empty() || channel_id == "0" {
        return Err(anyhow::anyhow!("Invalid channel ID: {}", channel_id));
    }

    let http = get_http_client(token).await;
    let c_id = channel_id
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid channel ID: {}", channel_id))?;

    let mut embed = serde_json::json!({
        "title": title,
        "description": description,
    });
    if let Some(color) = color {
        embed["color"] = serde_json::json!(color);
    }

    let map = serde_json::json!({
        "embeds": [embed]
    });
    let msg = http.send_message(c_id.into(), vec![], &map).await?;
    Ok(msg)
}

pub async fn send_file_attachment(
    token: &str,
    channel_id: &str,
    file_path: &Path,
) -> anyhow::Result<serenity::model::channel::Message> {
    if token.is_empty() || channel_id.is_empty() || channel_id == "0" {
        return Err(anyhow::anyhow!("Invalid parameters for file upload"));
    }

    if !file_path.exists() {
        return Err(anyhow::anyhow!("File not found: {:?}", file_path));
    }

    println!(
        "📡 Uploading file {:?} to Discord channel {}...",
        file_path, channel_id
    );

    let http = get_http_client(token).await;
    let c_id = channel_id
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid channel ID: {}", channel_id))?;

    let attachment = CreateAttachment::path(file_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create attachment: {}", e))?;

    let map = serde_json::json!({ "content": format!("📎 Attached file: `{}`", file_path.file_name().and_then(|s| s.to_str()).unwrap_or("file")) });
    let msg = http
        .send_message(c_id.into(), vec![attachment], &map)
        .await?;

    Ok(msg)
}

pub async fn broadcast_typing(token: &str, channel_id: &str) -> anyhow::Result<()> {
    if token.is_empty() || channel_id.is_empty() || channel_id == "0" {
        return Ok(());
    }

    let http = get_http_client(token).await;
    let c_id = channel_id
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid channel ID: {}", channel_id))?;

    http.broadcast_typing(c_id.into()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{split_code_block_chunks, split_message_chunks};

    #[test]
    fn test_split_message_chunks_prefers_newline_boundaries() {
        let content = "line one\nline two\nline three";
        let chunks = split_message_chunks(content, 12);

        assert_eq!(chunks, vec!["line one", "line two", "line three"]);
    }

    #[test]
    fn test_split_message_chunks_falls_back_to_hard_cut_without_newlines() {
        let content = "abcdefghijklmnopqrstuvwxyz";
        let chunks = split_message_chunks(content, 10);

        assert_eq!(chunks, vec!["abcdefghij", "klmnopqrst", "uvwxyz"]);
    }

    #[test]
    fn test_split_code_block_chunks_reserves_space_for_fences() {
        let content = "abcdefghijklmnopqrstuvwxyz";
        let chunks = split_code_block_chunks(content, "rust", 16);

        assert_eq!(
            chunks,
            vec!["abcd", "efgh", "ijkl", "mnop", "qrst", "uvwx", "yz"]
        );
    }
}
