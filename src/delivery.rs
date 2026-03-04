/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/delivery.rs
 * Responsibility: Discord delivery tools and artifact handoff.
 */

use crate::config::Config;
use crate::discord::client as discord_client;
use crate::tools::{ToolExecutionResult, is_path_safe};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
        .ok_or_else(|| {
            ToolExecutionResult::error(format!("Error: Missing required argument `{}`.", field))
        })
}

fn require_string_array_arg(args: &Value, field: &str) -> Result<Vec<String>, ToolExecutionResult> {
    let values = args.get(field).and_then(Value::as_array).ok_or_else(|| {
        ToolExecutionResult::error(format!("Error: Missing required argument `{}`.", field))
    })?;

    let mut items = Vec::with_capacity(values.len());
    for value in values {
        let item = value.as_str().ok_or_else(|| {
            ToolExecutionResult::error(format!(
                "Error: `{}` must contain only non-empty string values.",
                field
            ))
        })?;

        if item.is_empty() {
            return Err(ToolExecutionResult::error(format!(
                "Error: `{}` must contain only non-empty string values.",
                field
            )));
        }

        items.push(item.to_string());
    }

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

    let rel_path = requested_path
        .strip_prefix("guild/")
        .unwrap_or(requested_path);
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

fn build_outbox_file(
    base_path: &Path,
    filename: &str,
    content: &str,
) -> Result<PathBuf, ToolExecutionResult> {
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

fn path_label(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(fallback)
        .to_string()
}

fn delivery_error(action: &str, error: impl std::fmt::Display) -> ToolExecutionResult {
    ToolExecutionResult::error(format!("Error {}: {}", action, error))
}

fn delivery_success(message: impl Into<String>) -> ToolExecutionResult {
    ToolExecutionResult::success(message)
}

async fn send_attachment_file(
    token: &str,
    channel_id: &str,
    path: &Path,
    success_noun: &str,
    error_action: &str,
    fallback_name: &str,
) -> ToolExecutionResult {
    match discord_client::send_file_attachment(token, channel_id, path).await {
        Ok(_) => delivery_success(format!(
            "Sent {} `{}` to the current Discord channel.",
            success_noun,
            path_label(path, fallback_name)
        )),
        Err(error) => delivery_error(error_action, error),
    }
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

            match discord_client::send_bot_message(&config.discord.token, channel_id, content).await
            {
                Ok(_) => delivery_success("Sent text message to the current Discord channel."),
                Err(error) => delivery_error("sending message", error),
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

            match discord_client::send_reply_message(
                &config.discord.token,
                channel_id,
                message_id,
                content,
            )
            .await
            {
                Ok(_) => delivery_success("Sent reply to the current Discord channel."),
                Err(error) => delivery_error("sending reply", error),
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

            match discord_client::send_embed_message(
                &config.discord.token,
                channel_id,
                title,
                description,
                color,
            )
            .await
            {
                Ok(_) => delivery_success("Sent embed to the current Discord channel."),
                Err(error) => delivery_error("sending embed", error),
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

            send_attachment_file(
                &config.discord.token,
                channel_id,
                &resolved_path,
                "attachment",
                "sending attachment",
                "file",
            )
            .await
        }
        "send_attachments" => {
            let requested_paths = match require_string_array_arg(args, "paths") {
                Ok(paths) => paths,
                Err(err) => return Some(err),
            };

            let mut resolved_paths = Vec::with_capacity(requested_paths.len());
            for requested_path in &requested_paths {
                let resolved_path = match resolve_attachment_path(requested_path, base_path, config)
                {
                    Ok(path) => path,
                    Err(err) => return Some(err),
                };
                resolved_paths.push((requested_path, resolved_path));
            }

            let mut sent = Vec::new();
            for (requested_path, resolved_path) in resolved_paths {
                match discord_client::send_file_attachment(
                    &config.discord.token,
                    channel_id,
                    &resolved_path,
                )
                .await
                {
                    Ok(_) => sent.push(path_label(&resolved_path, "file")),
                    Err(e) => {
                        return Some(ToolExecutionResult::error(format!(
                            "Error sending attachment `{}`: {}",
                            requested_path, e
                        )));
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

            send_attachment_file(
                &config.discord.token,
                channel_id,
                &resolved_path,
                "image",
                "sending image",
                "image",
            )
            .await
        }
        "send_code_block" => {
            let content = match require_string_arg(args, "content") {
                Ok(content) => content,
                Err(err) => return Some(err),
            };
            let language = args.get("language").and_then(Value::as_str).unwrap_or("");

            match discord_client::send_code_block_message(
                &config.discord.token,
                channel_id,
                content,
                language,
            )
            .await
            {
                Ok(_) => delivery_success("Sent code block to the current Discord channel."),
                Err(error) => delivery_error("sending code block", error),
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

            match discord_client::send_file_attachment(
                &config.discord.token,
                channel_id,
                &outbox_file,
            )
            .await
            {
                Ok(_) => {
                    let label = path_label(&outbox_file, "artifact.txt");
                    if let Err(error) = fs::remove_file(&outbox_file) {
                        eprintln!(
                            "⚠️ Failed to remove sent outbox artifact {}: {}",
                            outbox_file.display(),
                            error
                        );
                    }

                    delivery_success(format!(
                        "Wrote and sent `{}` to the current Discord channel.",
                        label
                    ))
                }
                Err(error) => delivery_error("sending text file", error),
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

    #[test]
    fn test_path_label_uses_fallback_when_file_name_is_missing() {
        assert_eq!(path_label(Path::new("/"), "artifact.txt"), "artifact.txt");
    }

    #[test]
    fn test_require_string_array_arg_rejects_empty_items() {
        let result = require_string_array_arg(&json!({ "paths": ["", ""] }), "paths");

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .output
                .contains("must contain only non-empty string values")
        );
    }

    #[test]
    fn test_require_string_array_arg_rejects_non_string_items() {
        let result = require_string_array_arg(&json!({ "paths": ["ok.txt", 123] }), "paths");

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .output
                .contains("must contain only non-empty string values")
        );
    }

    #[test]
    fn test_resolve_attachment_path_accepts_guild_prefix() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join("docs").join("note.txt"), "hello").unwrap();

        let resolved =
            resolve_attachment_path("guild/docs/note.txt", dir.path(), &test_config()).unwrap();

        assert_eq!(resolved, dir.path().join("docs").join("note.txt"));
    }

    #[test]
    fn test_resolve_attachment_path_rejects_path_escape() {
        let dir = tempdir().unwrap();

        let result = resolve_attachment_path("../secret.txt", dir.path(), &test_config());

        assert!(result.is_err());
        assert!(result.unwrap_err().output.contains("Access denied"));
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
    async fn test_send_attachments_prevalidates_all_paths_before_sending() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ok.txt"), "hello").unwrap();

        let result = dispatch_delivery_tool(
            "send_attachments",
            &json!({ "paths": ["ok.txt", "missing.txt"] }),
            dir.path(),
            &test_config(),
            "123",
        )
        .await
        .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("File not found: missing.txt"));
    }

    #[tokio::test]
    async fn test_send_image_rejects_missing_path() {
        let dir = tempdir().unwrap();
        let result =
            dispatch_delivery_tool("send_image", &json!({}), dir.path(), &test_config(), "123")
                .await
                .unwrap();

        assert!(result.is_error);
        assert!(result.output.contains("Missing required argument `path`"));
    }

    #[tokio::test]
    async fn test_send_message_rejects_missing_content() {
        let dir = tempdir().unwrap();
        let result = dispatch_delivery_tool(
            "send_message",
            &json!({}),
            dir.path(),
            &test_config(),
            "123",
        )
        .await
        .unwrap();

        assert!(result.is_error);
        assert!(
            result
                .output
                .contains("Missing required argument `content`")
        );
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
        assert!(
            result
                .output
                .contains("Missing required argument `messageId`")
        );
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
        assert!(
            result
                .output
                .contains("Missing required argument `description`")
        );
    }
}
