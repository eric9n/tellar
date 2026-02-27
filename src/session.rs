/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/session.rs
 * Responsibility: Assemble role sessions from prompts, memory, and multimodal context.
 */

use crate::agent_loop::run_agent_loop;
use crate::config::Config;
use crate::context::{extract_and_load_images, load_unified_prompt, parse_task_document};
use crate::discord;
use crate::llm;
use std::fs;
use std::path::Path;

pub(crate) async fn run_react_loop(
    task: &str,
    full_context: &str,
    path: &Path,
    base_path: &Path,
    config: &Config,
    channel_id: &str,
) -> anyhow::Result<String> {
    let mut system_prompt_str = load_unified_prompt(base_path, channel_id);

    let mut channel_memory = String::new();
    if let Some((header, _)) = parse_task_document(full_context) {
        if let Some(origin_id) = header.origin_channel {
            if let Some(robust_folder) = discord::resolve_folder_by_id(base_path, &origin_id) {
                let knowledge_path = base_path
                    .join("channels")
                    .join(&robust_folder)
                    .join("KNOWLEDGE.md");
                if knowledge_path.exists() {
                    println!(
                        "🧠 Ritual linked to current channel folder: #{} (ID: {}), loading knowledge...",
                        robust_folder, origin_id
                    );
                    channel_memory = fs::read_to_string(knowledge_path).unwrap_or_default();
                }
            } else {
                println!(
                    "⚠️ Ritual origin channel (ID: {}) not found locally, skipping channel-specific knowledge.",
                    origin_id
                );
            }
        }
    }

    if let Some(parent) = path.parent() {
        let local_knowledge = parent.join("KNOWLEDGE.md");
        if local_knowledge.exists() {
            let local_mem = fs::read_to_string(local_knowledge).unwrap_or_default();
            channel_memory.push_str("\n\n### Ritual Local Knowledge:\n");
            channel_memory.push_str(&local_mem);
        }
    }

    let brain_knowledge_path = base_path.join("brain").join("KNOWLEDGE.md");
    let global_memory = if brain_knowledge_path.exists() {
        fs::read_to_string(brain_knowledge_path).unwrap_or_default()
    } else {
        String::new()
    };

    system_prompt_str.push_str(&format!(
        "\n\n### Semantic Memory (Channel):\n{}\n\n### Semantic Memory (Global):\n{}",
        channel_memory, global_memory
    ));

    let mut initial_messages = vec![llm::Message {
        role: llm::MessageRole::User,
        parts: vec![llm::MultimodalPart::text(format!(
            "### Current Blackboard Context:\n{}\n\n### Your Objective:\nYou are currently processing the step: \"{}\".\nUse native tool calling. Prefer `find` when the path is unknown, `ls` when the directory is known, then `grep` to narrow matches, then `read` before `write` or `edit`. Use a discovered skill only when the task needs domain-specific or external capabilities. Use `finish` when the step is complete.",
            full_context, task
        ))],
    }];

    let mut image_parts = extract_and_load_images(full_context, base_path);
    if !image_parts.is_empty() {
        initial_messages[0].parts.append(&mut image_parts);
    }

    run_agent_loop(
        initial_messages,
        path,
        base_path,
        config,
        channel_id,
        &system_prompt_str,
    )
    .await
}

pub(crate) async fn run_conversational_loop(
    full_context: &str,
    path: &Path,
    base_path: &Path,
    config: &Config,
    trigger_id: Option<String>,
    channel_id: &str,
) -> anyhow::Result<String> {
    let mut system_prompt_str = load_unified_prompt(base_path, channel_id);

    let mut channel_memory = String::new();
    if let Some(parent) = path.parent() {
        let knowledge_path = parent.join("KNOWLEDGE.md");
        if knowledge_path.exists() {
            channel_memory = fs::read_to_string(knowledge_path).unwrap_or_default();
        }
    }
    system_prompt_str.push_str(&format!(
        "\n\n### Semantic Memory (Channel Knowledge):\n{}",
        channel_memory
    ));

    let anchor = "> [Tellar]";
    let guidance = if let Some(pos) = full_context.rfind(anchor) {
        let increment = &full_context[pos..];
        if let Some(msg_start) = increment.find("\n---\n**Author**") {
            increment[msg_start..].trim().to_string()
        } else {
            "Check for follow-up or ritual steps.".to_string()
        }
    } else {
        full_context.to_string()
    };

    let mut trigger_instruction = guidance;
    if let Some(id) = trigger_id {
        trigger_instruction.push_str(&format!("\nSpecifically, the trigger message has ID: {}.", id));
    }

    let mut initial_messages = vec![llm::Message {
        role: llm::MessageRole::User,
        parts: vec![llm::MultimodalPart::text(format!(
            "### Current User Input (Specific Target):\n{}\n\nRespond naturally. Use Markdown. Prefer local cognition tools (`find`, `ls`, `grep`, `read`) before modifying files or invoking skills. Concise yet premium.",
            trigger_instruction
        ))],
    }];

    let mut image_parts = extract_and_load_images(full_context, base_path);
    if !image_parts.is_empty() {
        initial_messages[0].parts.append(&mut image_parts);
    }

    run_agent_loop(
        initial_messages,
        path,
        base_path,
        config,
        channel_id,
        &system_prompt_str,
    )
    .await
}
