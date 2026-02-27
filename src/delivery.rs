/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/delivery.rs
 * Responsibility: Discord delivery tools and artifact handoff.
 */

use crate::config::Config;
use crate::discord;
use crate::tools::{is_path_safe, ToolExecutionResult};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const DELIVERY_TOOL_NAMES: &[&str] = &[
    "send_message",
    "send_reply",
    "send_embed",
    "send_attachment",
    "send_attachments",
    "send_image",
    "send_code_block",
    "send_text_file",
];

pub(crate) fn delivery_tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "send_message",
            "description": "Send text to the current Discord channel. Large messages are chunked automatically with newline-aware splitting.",
            "parameters": {
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The text content to send to the current Discord channel" }
                },
                "required": ["content"]
            }
        }),
        json!({
            "name": "send_reply",
            "description": "Send a reply to a specific Discord message in the current channel.",
            "parameters": {
                "type": "object",
                "properties": {
                    "messageId": { "type": "string", "description": "The Discord message ID to reply to" },
                    "content": { "type": "string", "description": "The reply content" }
                },
                "required": ["messageId", "content"]
            }
        }),
        json!({
            "name": "send_embed",
            "description": "Send a simple rich embed to the current Discord channel.",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Embed title" },
                    "description": { "type": "string", "description": "Embed description" },
                    "color": { "type": "number", "description": "Optional decimal RGB color, such as 3447003" }
                },
                "required": ["title", "description"]
            }
        }),
        json!({
            "name": "send_attachment",
            "description": "Send a local file to the current Discord channel as an attachment. Relative paths resolve from the guild root. Absolute host paths require runtime.privileged=true.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Local file path to send" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "send_attachments",
            "description": "Send multiple local files to the current Discord channel as separate attachments.",
            "parameters": {
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of local file paths to send"
                    }
                },
                "required": ["paths"]
            }
        }),
        json!({
            "name": "send_image",
            "description": "Send an image file to the current Discord channel. This is a semantic alias for image-focused attachment delivery.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Local image path to send" }
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "send_code_block",
            "description": "Send formatted code or logs as a fenced code block to the current Discord channel.",
            "parameters": {
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The code or log text to send" },
                    "language": { "type": "string", "description": "Optional code fence language, such as 'python' or 'bash'" }
                },
                "required": ["content"]
            }
        }),
        json!({
            "name": "send_text_file",
            "description": "Write text into a local outbox file and send it as a Discord attachment. Use this when content is too large for a normal message.",
            "parameters": {
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The text content to write into the file" },
                    "filename": { "type": "string", "description": "Optional attachment filename. Defaults to 'artifact.txt'" }
                },
                "required": ["content"]
            }
        }),
    ]
}

fn require_string_arg<'a>(args: &'a Value, field: &str) -> Result<&'a str, ToolExecutionResult> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolExecutionResult::error(format!("Error: Missing required argument `{}`.", field)))
}

fn require_string_array_arg(args: &Value, field: &str) -> Result<Vec<String>, ToolExecutionResult> {
    let values = args
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| ToolExecutionResult::error(format!("Error: Missing required argument `{}`.", field)))?;

    let items = values
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if items.is_empty() {
        Err(ToolExecutionResult::error(format!(
            "Error: `{}` must contain at least one non-empty path.",
            field
        )))
    } else {
        Ok(items)
    }
}

fn resolve_attachment_path(
    requested_path: &str,
    base_path: &Path,
    config: &Config,
) -> Result<PathBuf, ToolExecutionResult> {
    if requested_path.starts_with('/') {
        if !config.runtime.privileged {
            return Err(ToolExecutionResult::error(
                "Error: Absolute host paths for attachments require runtime.privileged=true.",
            ));
        }

        let path = PathBuf::from(requested_path);
        if !path.exists() {
            return Err(ToolExecutionResult::error(format!(
                "Error: File not found: {}",
                requested_path
            )));
        }
        if !path.is_file() {
            return Err(ToolExecutionResult::error(format!(
                "Error: Attachment path is not a file: {}",
                requested_path
            )));
        }

        return Ok(path);
    }

    let rel_path = requested_path.strip_prefix("guild/").unwrap_or(requested_path);
    let rel_path = rel_path.strip_prefix("./").unwrap_or(rel_path);

    if !is_path_safe(base_path, rel_path) {
        return Err(ToolExecutionResult::error(
            "Error: Access denied. Path must be within the guild directory.",
        ));
    }

    let full_path = base_path.join(rel_path);
    if !full_path.exists() {
        return Err(ToolExecutionResult::error(format!(
            "Error: File not found: {}",
            rel_path
        )));
    }
    if !full_path.is_file() {
        return Err(ToolExecutionResult::error(format!(
            "Error: Attachment path is not a file: {}",
            rel_path
        )));
    }

    Ok(full_path)
}

fn sanitize_filename(name: &str) -> String {
    let basename = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact.txt");
    let cleaned: String = basename
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if cleaned.is_empty() {
        "artifact.txt".to_string()
    } else {
        cleaned
    }
}

fn build_outbox_file(base_path: &Path, filename: &str, content: &str) -> Result<PathBuf, ToolExecutionResult> {
    let outbox = base_path.join("brain").join("outbox");
    fs::create_dir_all(&outbox)
        .map_err(|e| ToolExecutionResult::error(format!("Error creating outbox: {}", e)))?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let sanitized = sanitize_filename(filename);
    let final_path = outbox.join(format!("{}_{}", timestamp, sanitized));
    fs::write(&final_path, content)
        .map_err(|e| ToolExecutionResult::error(format!("Error writing outbox file: {}", e)))?;
    Ok(final_path)
}

pub(crate) async fn dispatch_delivery_tool(
    name: &str,
    args: &Value,
    base_path: &Path,
    config: &Config,
    channel_id: &str,
) -> Option<ToolExecutionResult> {
    let result = match name {
        "send_message" => {
            let content = match require_string_arg(args, "content") {
                Ok(content) => content,
                Err(err) => return Some(err),
            };

            match discord::send_bot_message(&config.discord.token, channel_id, content).await {
                Ok(_) => ToolExecutionResult::success("Sent text message to the current Discord channel."),
                Err(e) => ToolExecutionResult::error(format!("Error sending message: {}", e)),
            }
        }
        "send_reply" => {
            let message_id = match require_string_arg(args, "messageId") {
                Ok(message_id) => message_id,
                Err(err) => return Some(err),
            };
            let content = match require_string_arg(args, "content") {
                Ok(content) => content,
                Err(err) => return Some(err),
            };

            match discord::send_reply_message(
                &config.discord.token,
                channel_id,
                message_id,
                content,
            )
            .await
            {
                Ok(_) => ToolExecutionResult::success("Sent reply to the current Discord channel."),
                Err(e) => ToolExecutionResult::error(format!("Error sending reply: {}", e)),
            }
        }
        "send_embed" => {
            let title = match require_string_arg(args, "title") {
                Ok(title) => title,
                Err(err) => return Some(err),
            };
            let description = match require_string_arg(args, "description") {
                Ok(description) => description,
                Err(err) => return Some(err),
            };
            let color = args
                .get("color")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok());

            match discord::send_embed_message(
                &config.discord.token,
                channel_id,
                title,
                description,
                color,
            )
            .await
            {
                Ok(_) => ToolExecutionResult::success("Sent embed to the current Discord channel."),
                Err(e) => ToolExecutionResult::error(format!("Error sending embed: {}", e)),
            }
        }
        "send_attachment" => {
            let requested_path = match require_string_arg(args, "path") {
                Ok(path) => path,
                Err(err) => return Some(err),
            };
            let resolved_path = match resolve_attachment_path(requested_path, base_path, config) {
                Ok(path) => path,
                Err(err) => return Some(err),
            };

            match discord::send_file_attachment(&config.discord.token, channel_id, &resolved_path).await {
                Ok(_) => ToolExecutionResult::success(format!(
                    "Sent attachment `{}` to the current Discord channel.",
                    resolved_path
                        .file_name()
                        .and_then(|v| v.to_str())
                        .unwrap_or("file")
                )),
                Err(e) => ToolExecutionResult::error(format!("Error sending attachment: {}", e)),
            }
        }
        "send_attachments" => {
            let requested_paths = match require_string_array_arg(args, "paths") {
                Ok(paths) => paths,
                Err(err) => return Some(err),
            };

            let mut sent = Vec::new();
            for requested_path in requested_paths {
                let resolved_path = match resolve_attachment_path(&requested_path, base_path, config) {
                    Ok(path) => path,
                    Err(err) => return Some(err),
                };

                match discord::send_file_attachment(&config.discord.token, channel_id, &resolved_path).await {
                    Ok(_) => {
                        sent.push(
                            resolved_path
                                .file_name()
                                .and_then(|v| v.to_str())
                                .unwrap_or("file")
                                .to_string(),
                        );
                    }
                    Err(e) => {
                        return Some(ToolExecutionResult::error(format!(
                            "Error sending attachment `{}`: {}",
                            requested_path, e
                        )))
                    }
                }
            }

            ToolExecutionResult::success(format!(
                "Sent {} attachment(s): {}",
                sent.len(),
                sent.join(", ")
            ))
        }
        "send_image" => {
            let requested_path = match require_string_arg(args, "path") {
                Ok(path) => path,
                Err(err) => return Some(err),
            };
            let resolved_path = match resolve_attachment_path(requested_path, base_path, config) {
                Ok(path) => path,
                Err(err) => return Some(err),
            };

            match discord::send_file_attachment(&config.discord.token, channel_id, &resolved_path).await {
                Ok(_) => ToolExecutionResult::success(format!(
                    "Sent image `{}` to the current Discord channel.",
                    resolved_path
                        .file_name()
                        .and_then(|v| v.to_str())
                        .unwrap_or("image")
                )),
                Err(e) => ToolExecutionResult::error(format!("Error sending image: {}", e)),
            }
        }
        "send_code_block" => {
            let content = match require_string_arg(args, "content") {
                Ok(content) => content,
                Err(err) => return Some(err),
            };
            let language = args.get("language").and_then(Value::as_str).unwrap_or("");
            let fenced = if language.is_empty() {
                format!("```\n{}\n```", content)
            } else {
                format!("```{}\n{}\n```", language, content)
            };

            match discord::send_bot_message(&config.discord.token, channel_id, &fenced).await {
                Ok(_) => ToolExecutionResult::success("Sent code block to the current Discord channel."),
                Err(e) => ToolExecutionResult::error(format!("Error sending code block: {}", e)),
            }
        }
        "send_text_file" => {
            let content = match require_string_arg(args, "content") {
                Ok(content) => content,
                Err(err) => return Some(err),
            };
            let filename = args
                .get("filename")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("artifact.txt");
            let outbox_file = match build_outbox_file(base_path, filename, content) {
                Ok(path) => path,
                Err(err) => return Some(err),
            };

            match discord::send_file_attachment(&config.discord.token, channel_id, &outbox_file).await {
                Ok(_) => ToolExecutionResult::success(format!(
                    "Wrote and sent `{}` to the current Discord channel.",
                    outbox_file
                        .file_name()
                        .and_then(|v| v.to_str())
                        .unwrap_or("artifact.txt")
                )),
                Err(e) => ToolExecutionResult::error(format!("Error sending text file: {}", e)),
            }
        }
        _ => return None,
    };

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DiscordConfig, GeminiConfig, RuntimeConfig};
    use tempfile::tempdir;

    fn test_config() -> Config {
        Config {
            gemini: GeminiConfig {
                api_key: "fake".to_string(),
                model: "fake".to_string(),
            },
            discord: DiscordConfig {
                token: "fake".to_string(),
                guild_id: None,
                channel_mappings: None,
            },
            runtime: RuntimeConfig::default(),
            guardian: None,
        }
    }

    #[test]
    fn test_sanitize_filename_strips_path_and_replaces_unsafe_chars() {
        assert_eq!(sanitize_filename("../bad name?.txt"), "bad_name_.txt");
    }

    #[test]
    fn test_build_outbox_file_writes_content() {
        let dir = tempdir().unwrap();
        let file = build_outbox_file(dir.path(), "notes.txt", "hello").unwrap();
        let written = fs::read_to_string(file).unwrap();
        assert_eq!(written, "hello");
    }

    #[tokio::test]
    async fn test_send_attachment_rejects_absolute_path_when_privileged_mode_is_disabled() {
        let dir = tempdir().unwrap();
        let result = dispatch_delivery_tool(
            "send_attachment",
            &json!({ "path": "/tmp/example.txt" }),
            dir.path(),
            &test_config(),
            "123",
        )
        .await
        .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("runtime.privileged=true"));
    }

    #[tokio::test]
    async fn test_send_attachments_rejects_missing_paths() {
        let dir = tempdir().unwrap();
        let result = dispatch_delivery_tool(
            "send_attachments",
            &json!({}),
            dir.path(),
            &test_config(),
            "123",
        )
        .await
        .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("Missing required argument `paths`"));
    }

    #[tokio::test]
    async fn test_send_image_rejects_missing_path() {
        let dir = tempdir().unwrap();
        let result = dispatch_delivery_tool(
            "send_image",
            &json!({}),
            dir.path(),
            &test_config(),
            "123",
        )
        .await
        .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("Missing required argument `path`"));
    }

    #[tokio::test]
    async fn test_send_message_rejects_missing_content() {
        let dir = tempdir().unwrap();
        let result = dispatch_delivery_tool("send_message", &json!({}), dir.path(), &test_config(), "123")
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("Missing required argument `content`"));
    }

    #[tokio::test]
    async fn test_send_reply_rejects_missing_message_id() {
        let dir = tempdir().unwrap();
        let result = dispatch_delivery_tool(
            "send_reply",
            &json!({ "content": "hello" }),
            dir.path(),
            &test_config(),
            "123",
        )
        .await
        .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("Missing required argument `messageId`"));
    }

    #[tokio::test]
    async fn test_send_embed_rejects_missing_description() {
        let dir = tempdir().unwrap();
        let result = dispatch_delivery_tool(
            "send_embed",
            &json!({ "title": "Notice" }),
            dir.path(),
            &test_config(),
            "123",
        )
        .await
        .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("Missing required argument `description`"));
    }
}
