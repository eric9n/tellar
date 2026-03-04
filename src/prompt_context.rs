/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/prompt_context.rs
 * Responsibility: Prompt loading and prompt-related test helpers.
 */

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::SystemTime;

#[derive(Clone)]
struct CachedPrompt {
    base_modified: Option<SystemTime>,
    channel_modified: Option<SystemTime>,
    prompt: String,
}

static PROMPT_CACHE: Lazy<RwLock<HashMap<(PathBuf, String), CachedPrompt>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

fn file_modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// Loads the unified system prompt: Base AGENTS.md + optional <CHANNEL_ID>.AGENTS.md
pub(crate) fn load_unified_prompt(base_path: &Path, channel_id: &str) -> String {
    let agents_dir = base_path.join("agents");
    let base_prompt_path = agents_dir.join("AGENTS.md");
    let channel_prompt_path =
        (channel_id != "0").then(|| agents_dir.join(format!("{}.AGENTS.md", channel_id)));
    let base_modified = file_modified(&base_prompt_path);
    let channel_modified = channel_prompt_path.as_deref().and_then(file_modified);
    let cache_key = (base_path.to_path_buf(), channel_id.to_string());

    if let Some(cached) = PROMPT_CACHE
        .read()
        .ok()
        .and_then(|cache| cache.get(&cache_key).cloned())
    {
        if cached.base_modified == base_modified && cached.channel_modified == channel_modified {
            return cached.prompt;
        }
    }

    let mut system_prompt = fs::read_to_string(base_prompt_path)
        .unwrap_or_else(|_| "You are Tellar, a cyber steward.".to_string());

    if let Some(channel_prompt_path) = channel_prompt_path {
        if channel_prompt_path.exists() {
            if let Ok(channel_prompt) = fs::read_to_string(channel_prompt_path) {
                println!(
                    "🎭 Loading channel-specific identity for ID: {}",
                    channel_id
                );
                system_prompt.push_str("\n\n### Channel-Specific Identity:\n");
                system_prompt.push_str(&channel_prompt);
            }
        }
    }

    if let Ok(mut cache) = PROMPT_CACHE.write() {
        cache.insert(
            cache_key,
            CachedPrompt {
                base_modified,
                channel_modified,
                prompt: system_prompt.clone(),
            },
        );
    }

    system_prompt
}

#[cfg(test)]
use crate::llm;
#[cfg(test)]
use regex::Regex;

/// Reread the blackboard and inject any NEW messages into the history
#[cfg(test)]
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
            println!(
                "📥 Steering: New user message detected mid-loop: '{}'",
                new_msg
            );
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
