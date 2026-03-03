/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/conversation_context.rs
 * Responsibility: Assemble conversational and react-mode prompt context.
 */

use crate::config::Config;
use crate::context::extract_and_load_images;
use crate::conversation_policy::{
    conversational_agent_instruction, execution_boundary_note, react_objective_instruction,
};
use crate::discord;
use crate::llm;
use crate::skills::{build_relevant_skill_guidance, has_explicit_skill_match};
use std::fs;
use std::path::Path;

pub(crate) fn load_channel_knowledge(channel_path: &Path) -> String {
    let knowledge_path = channel_path.join("KNOWLEDGE.md");
    if knowledge_path.exists() {
        fs::read_to_string(knowledge_path).unwrap_or_default()
    } else {
        String::new()
    }
}

pub(crate) fn build_react_prompt_context(
    system_prompt: &mut String,
    task: &str,
    full_context: &str,
    path: &Path,
    base_path: &Path,
    config: &Config,
) -> Vec<llm::Message> {
    let mut channel_memory = String::new();
    if let Some((header, _)) = crate::context::parse_task_document(full_context) {
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
        let local_mem = load_channel_knowledge(parent);
        if !local_mem.is_empty() {
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

    system_prompt.push_str(&format!(
        "\n\n### Semantic Memory (Channel):\n{}\n\n### Semantic Memory (Global):\n{}",
        channel_memory, global_memory
    ));

    let explicit_skill_match = has_explicit_skill_match(base_path, task);
    if let Some(skill_guidance) = build_relevant_skill_guidance(base_path, task) {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&skill_guidance);
    }

    let objective_instruction = react_objective_instruction(explicit_skill_match);
    let mut initial_messages = vec![llm::Message {
        role: llm::MessageRole::User,
        parts: vec![llm::MultimodalPart::text(format!(
            "### Current Blackboard Context:\n{}\n\n### Your Objective:\nYou are currently processing the step: \"{}\".\n{}",
            full_context, task, objective_instruction
        ))],
    }];

    if let Some(note) = execution_boundary_note(task, config) {
        initial_messages.push(llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(note)],
        });
    }

    let mut image_parts = extract_and_load_images(full_context, base_path);
    if !image_parts.is_empty() {
        initial_messages[0].parts.append(&mut image_parts);
    }

    initial_messages
}

pub(crate) fn build_conversational_agent_context(
    system_prompt: &mut String,
    workset_text: &str,
    full_context: &str,
    path: &Path,
    base_path: &Path,
    config: &Config,
) -> Vec<llm::Message> {
    if let Some(parent) = path.parent() {
        let channel_memory = load_channel_knowledge(parent);
        system_prompt.push_str(&format!(
            "\n\n### Semantic Memory (Channel Knowledge):\n{}",
            channel_memory
        ));
    }

    let explicit_skill_match = has_explicit_skill_match(base_path, workset_text);
    if let Some(skill_guidance) = build_relevant_skill_guidance(base_path, workset_text) {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&skill_guidance);
    }

    let response_instruction = conversational_agent_instruction(explicit_skill_match);
    let mut initial_messages = vec![llm::Message {
        role: llm::MessageRole::User,
        parts: vec![llm::MultimodalPart::text(format!(
            "### Current User Input (Specific Target):\n{}\n\n{}",
            workset_text, response_instruction
        ))],
    }];

    if let Some(note) = execution_boundary_note(workset_text, config) {
        initial_messages.push(llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(note)],
        });
    }

    let mut image_parts = extract_and_load_images(full_context, base_path);
    if !image_parts.is_empty() {
        initial_messages[0].parts.append(&mut image_parts);
    }

    initial_messages
}
