/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/context.rs
 * Responsibility: Prompt loading, blackboard parsing, and steering/context helpers.
 */

use crate::discord;
use crate::llm;
use base64::{engine::general_purpose, Engine as _};
use regex::Regex;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub(crate) struct TaskHeader {
    pub(crate) status: String,
    pub(crate) schedule: Option<String>,
    pub(crate) injection_template: Option<String>,
    pub(crate) origin_channel: Option<String>,
}

/// Loads the unified system prompt: Base AGENTS.md + optional <CHANNEL_ID>.AGENTS.md
pub(crate) fn load_unified_prompt(base_path: &Path, channel_id: &str) -> String {
    let agents_dir = base_path.join("agents");
    let base_prompt_path = agents_dir.join("AGENTS.md");

    let mut system_prompt = fs::read_to_string(base_prompt_path)
        .unwrap_or_else(|_| "You are Tellar, a cyber steward.".to_string());

    if channel_id != "0" {
        let channel_prompt_path = agents_dir.join(format!("{}.AGENTS.md", channel_id));
        if channel_prompt_path.exists() {
            if let Ok(channel_prompt) = fs::read_to_string(channel_prompt_path) {
                println!("🎭 Loading channel-specific identity for ID: {}", channel_id);
                system_prompt.push_str("\n\n### Channel-Specific Identity:\n");
                system_prompt.push_str(&channel_prompt);
            }
        }
    }

    system_prompt
}

pub(crate) fn extract_and_load_images(text: &str, base_path: &Path) -> Vec<llm::MultimodalPart> {
    let mut parts = Vec::new();
    let re_local = Regex::new(r"\(local: \[file://(.*?)\]\)").unwrap();

    for caps in re_local.captures_iter(text) {
        if let Some(rel_path) = caps.get(1) {
            let full_path = base_path.join(rel_path.as_str());
            if full_path.exists() {
                if let Ok(data) = fs::read(&full_path) {
                    let b64 = general_purpose::STANDARD.encode(data);
                    let ext = full_path.extension().and_then(|s| s.to_str()).unwrap_or("png");
                    let mime = match ext {
                        "png" => "image/png",
                        "jpg" | "jpeg" => "image/jpeg",
                        "gif" => "image/gif",
                        "webp" => "image/webp",
                        _ => "image/png",
                    };
                    println!("👁️ Loading local image for LLM: {:?}", full_path.file_name().unwrap());
                    parts.push(llm::MultimodalPart::image(mime, b64));
                }
            }
        }
    }

    parts
}

pub(crate) fn parse_task_document(content: &str) -> Option<(TaskHeader, &str)> {
    if !content.starts_with("---") {
        return None;
    }
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        return None;
    }
    let yaml_str = parts[1];
    let body = parts[2].trim();
    if let Ok(header) = serde_yaml::from_str::<TaskHeader>(yaml_str) {
        Some((header, body))
    } else {
        None
    }
}

pub(crate) fn is_conversational_log(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    file_name.len() == 13
        && file_name.chars().nth(4) == Some('-')
        && file_name.chars().nth(7) == Some('-')
        && path.extension().and_then(|s| s.to_str()) == Some("md")
}

pub(crate) fn extract_channel_id_from_path(path: &Path) -> String {
    if let Ok(content) = fs::read_to_string(path) {
        if let Some((header, _)) = parse_task_document(&content) {
            if let Some(origin) = header.origin_channel {
                if origin != "0" {
                    return origin;
                }
            }
        }
    }

    if let Some(parent) = path.parent() {
        if let Some(folder_name) = parent.file_name().and_then(|s| s.to_str()) {
            if let Some(id) = discord::extract_id_from_folder(folder_name) {
                return id;
            }
        }
    }

    "0".to_string()
}

/// Reread the blackboard and inject any NEW messages into the history
pub(crate) async fn update_history_with_steering(
    messages: &mut Vec<llm::Message>,
    path: &Path,
) -> anyhow::Result<()> {
    let current_content = fs::read_to_string(path).unwrap_or_default();

    let re_author = Regex::new(r"(?s)\*\*Author\*\*: (.*?) \| \*\*Time\*\*.*?\n\n(.*?)\n").unwrap();
    let mut blackboard_user_messages = Vec::new();
    for caps in re_author.captures_iter(&current_content) {
        let author = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let body = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if !author.contains("Tellar") {
            blackboard_user_messages.push(body.trim().to_string());
        }
    }

    let last_blackboard_msg = blackboard_user_messages.last();
    let last_history_msg = messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, llm::MessageRole::User))
        .and_then(|m| m.parts.first())
        .and_then(|p| p.text.as_ref());

    if let Some(new_msg) = last_blackboard_msg {
        if Some(new_msg) != last_history_msg {
            println!("📥 Steering: New user message detected mid-loop: '{}'", new_msg);
            messages.push(llm::Message {
                role: llm::MessageRole::User,
                parts: vec![llm::MultimodalPart::text(new_msg.clone())],
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn test_steering_detection() -> anyhow::Result<()> {
        let path = std::env::current_dir()?.join("test_blackboard.md");

        fs::write(&path, "**Author**: User1 | **Time**: 12:00\n\nHello\n")?;

        let mut messages = vec![llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text("Hello".to_string())],
        }];

        update_history_with_steering(&mut messages, &path).await?;
        assert_eq!(messages.len(), 1);

        fs::write(
            &path,
            "**Author**: User1 | **Time**: 12:00\n\nHello\n\n---\n**Author**: User1 | **Time**: 12:01\n\nSTOP!\n",
        )?;

        update_history_with_steering(&mut messages, &path).await?;

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, llm::MessageRole::User);
        assert_eq!(messages[1].parts[0].text.as_ref().unwrap(), "STOP!");

        let _ = fs::remove_file(&path);
        Ok(())
    }
}
