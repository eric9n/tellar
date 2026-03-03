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
use crate::skills::{build_relevant_skill_guidance, find_explicit_tool_match, has_explicit_skill_match};
use crate::tools::dispatch_tool;
use serde_json::{json, Value};
use regex::Regex;
use std::fs;
use std::path::Path;

fn unsupported_request_note(text: &str, config: &Config) -> Option<String> {
    let has_unix_abs_path = Regex::new(r#"(^|[\s`'"(])/(?:[^/\s]+/)*[^/\s]+"#)
        .unwrap()
        .is_match(text);
    let wants_attachment = text.contains("附件")
        || text.to_ascii_lowercase().contains("attachment")
        || text.to_ascii_lowercase().contains("attach ");

    let mut constraints = Vec::new();
    if has_unix_abs_path {
        let exec_guidance = if config.runtime.privileged {
            "This request targets a host path. Use `exec` first instead of searching with guild file tools."
        } else {
            "This request targets a host path. Call `exec` first; it will reject immediately because privileged mode is disabled, then explain the limitation instead of searching with guild file tools."
        };
        constraints.push(exec_guidance.to_string());
    }
    if wants_attachment {
        constraints.push(
            "The user explicitly wants a file attachment. If you obtain a local file path, use `send_attachment` to deliver it to the current Discord channel. Do not paste the full file contents as a substitute unless the user changes the request.".to_string(),
        );
    }

    if constraints.is_empty() {
        None
    } else {
        Some(format!(
            "### Execution Boundary\n{}\nIf this request depends on unsupported capabilities, say so directly and finish instead of continuing to search.",
            constraints.join("\n")
        ))
    }
}

#[derive(Debug)]
enum ConversationalDispatch {
    DirectTool { tool_name: String, args: Value },
    UnsupportedRealtime { reason: String },
}

fn extract_symbol(text: &str) -> Option<String> {
    let full = Regex::new(r"\b([A-Z]{1,8}\.(?:US|HK|CN))\b").unwrap();
    if let Some(caps) = full.captures(text) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }

    let bare = Regex::new(r"\b([A-Z]{1,6})\b").unwrap();
    bare.captures(text)
        .and_then(|caps| caps.get(1))
        .map(|m| format!("{}.US", m.as_str()))
}

fn extract_expiry(text: &str) -> Option<String> {
    let expiry = Regex::new(r"\b(20\d{2}-\d{2}-\d{2})\b").unwrap();
    expiry
        .captures(text)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

fn looks_like_realtime_external_query(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    text.contains("天气")
        || lowered.contains("weather")
        || lowered.contains("汇率")
        || lowered.contains("exchange rate")
        || lowered.contains("新闻")
        || lowered.contains("news")
}

fn classify_conversational_request(base_path: &Path, text: &str) -> Option<ConversationalDispatch> {
    if let Some(tool_name) = find_explicit_tool_match(base_path, text) {
        let args = match tool_name.as_str() {
            "stock_quote" | "option_expiries" | "probe" => {
                extract_symbol(text).map(|symbol| json!({ "symbol": symbol }))
            }
            "option_quote" | "analyze_option" => {
                extract_symbol(text).map(|symbol| json!({ "symbol": symbol }))
            }
            "option_chain" | "analyze_chain" | "market_tone" | "skew" | "smile" | "put_call_bias" => {
                match (extract_symbol(text), extract_expiry(text)) {
                    (Some(symbol), Some(expiry)) => Some(json!({ "symbol": symbol, "expiry": expiry })),
                    _ => None,
                }
            }
            "market_extreme" | "iv_rank" | "signal_history" => {
                extract_symbol(text).map(|symbol| json!({ "symbol": symbol }))
            }
            "relative_extreme" => {
                let symbol = extract_symbol(text)?;
                Some(json!({ "symbol": symbol, "benchmark": "QQQ.US" }))
            }
            _ => None,
        };

        if let Some(args) = args {
            return Some(ConversationalDispatch::DirectTool { tool_name, args });
        }
    }

    let lowered = text.to_ascii_lowercase();
    if (text.contains("股价") || lowered.contains("stock price") || lowered.contains("quote"))
        && !lowered.contains("option")
    {
        if let Some(symbol) = extract_symbol(text) {
            return Some(ConversationalDispatch::DirectTool {
                tool_name: "stock_quote".to_string(),
                args: json!({ "symbol": symbol }),
            });
        }
    }

    if (text.contains("到期日") || lowered.contains("expir"))
        && let Some(symbol) = extract_symbol(text)
    {
        return Some(ConversationalDispatch::DirectTool {
            tool_name: "option_expiries".to_string(),
            args: json!({ "symbol": symbol }),
        });
    }

    if looks_like_realtime_external_query(text) {
        return Some(ConversationalDispatch::UnsupportedRealtime {
            reason: "This looks like a real-time external information request, but no matching live data skill is installed for that category. I should say that directly instead of searching local files.".to_string(),
        });
    }

    None
}

async fn run_direct_conversational_dispatch(
    dispatch: ConversationalDispatch,
    base_path: &Path,
    config: &Config,
    channel_id: &str,
) -> anyhow::Result<String> {
    match dispatch {
        ConversationalDispatch::UnsupportedRealtime { reason } => Ok(format!(
            "I can't answer that reliably right now because I don't have a matching live data tool for that request. {}",
            reason
        )),
        ConversationalDispatch::DirectTool { tool_name, args } => {
            let result = dispatch_tool(&tool_name, &args, base_path, config, channel_id).await;
            if result.is_error {
                Ok(format!(
                    "I tried `{}` but it failed:\n{}",
                    tool_name, result.output
                ))
            } else {
                Ok(result.output)
            }
        }
    }
}

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

    let explicit_skill_match = has_explicit_skill_match(base_path, task);
    if let Some(skill_guidance) = build_relevant_skill_guidance(base_path, task) {
        system_prompt_str.push_str("\n\n");
        system_prompt_str.push_str(&skill_guidance);
    }

    let objective_instruction = if explicit_skill_match {
        "Use native tool calling. The user explicitly referenced a skill or skill tool. \
Prioritize the matching discovered skill/tool before generic file exploration. If the named \
skill/tool returns a useful result, answer the user directly instead of continuing with \
find/ls/grep/read. Only fall back to local cognition tools if the named skill cannot satisfy the request."
    } else {
        "Use native tool calling. Prefer `find` when the path is unknown, `ls` when the directory is known, then `grep` to narrow matches, then `read` before `write` or `edit`. Use a discovered skill only when the task needs domain-specific or external capabilities. Use `finish` when the step is complete."
    };

    let mut initial_messages = vec![llm::Message {
        role: llm::MessageRole::User,
        parts: vec![llm::MultimodalPart::text(format!(
            "### Current Blackboard Context:\n{}\n\n### Your Objective:\nYou are currently processing the step: \"{}\".\n{}",
            full_context, task, objective_instruction
        ))],
    }];

    if let Some(note) = unsupported_request_note(task, config) {
        initial_messages.push(llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(note)],
        });
    }

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
    let trigger_instruction = {
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

        let mut instruction = guidance;
        if let Some(id) = trigger_id.clone() {
            instruction.push_str(&format!("\nSpecifically, the trigger message has ID: {}.", id));
        }
        instruction
    };

    if let Some(dispatch) = classify_conversational_request(base_path, &trigger_instruction) {
        return run_direct_conversational_dispatch(dispatch, base_path, config, channel_id).await;
    }

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

    let explicit_skill_match = has_explicit_skill_match(base_path, &trigger_instruction);
    if let Some(skill_guidance) = build_relevant_skill_guidance(base_path, &trigger_instruction) {
        system_prompt_str.push_str("\n\n");
        system_prompt_str.push_str(&skill_guidance);
    }

    let response_instruction = if explicit_skill_match {
        "Respond naturally. Use Markdown. The user explicitly named a skill or tool, so prioritize that matching discovered skill/tool first. If it returns a usable result, answer immediately instead of exploring with find/ls/grep/read. Only use local cognition tools when the named skill is insufficient or fails and you need to explain why."
    } else {
        "Respond naturally. Use Markdown. Prefer local cognition tools (`find`, `ls`, `grep`, `read`) before modifying files or invoking skills. Concise yet premium."
    };

    let mut initial_messages = vec![llm::Message {
        role: llm::MessageRole::User,
        parts: vec![llm::MultimodalPart::text(format!(
            "### Current User Input (Specific Target):\n{}\n\n{}",
            trigger_instruction, response_instruction
        ))],
    }];

    if let Some(note) = unsupported_request_note(&trigger_instruction, config) {
        initial_messages.push(llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(note)],
        });
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_skill(base: &Path, dir_name: &str, body: &str) {
        let skill_dir = base.join("skills").join(dir_name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn test_unsupported_request_note_detects_absolute_path() {
        let note = unsupported_request_note(
            "请读取 /root/process_intel.py",
            &Config {
                gemini: crate::config::GeminiConfig {
                    api_key: "fake".to_string(),
                    model: "fake".to_string(),
                },
                discord: crate::config::DiscordConfig {
                    token: "fake".to_string(),
                    guild_id: None,
                    channel_mappings: None,
                },
                runtime: crate::config::RuntimeConfig::default(),
                guardian: None,
            },
        )
        .unwrap();
        assert!(note.contains("Call `exec` first"));
    }

    #[test]
    fn test_unsupported_request_note_detects_attachment_request() {
        let note = unsupported_request_note(
            "以附件发给我",
            &Config {
                gemini: crate::config::GeminiConfig {
                    api_key: "fake".to_string(),
                    model: "fake".to_string(),
                },
                discord: crate::config::DiscordConfig {
                    token: "fake".to_string(),
                    guild_id: None,
                    channel_mappings: None,
                },
                runtime: crate::config::RuntimeConfig::default(),
                guardian: None,
            },
        )
        .unwrap();
        assert!(note.contains("send_attachment"));
        assert!(note.contains("Do not paste the full file contents"));
    }

    #[test]
    fn test_unsupported_request_note_none_for_normal_request() {
        assert!(
            unsupported_request_note(
                "Read channels/general/KNOWLEDGE.md",
                &Config {
                    gemini: crate::config::GeminiConfig {
                        api_key: "fake".to_string(),
                        model: "fake".to_string(),
                    },
                    discord: crate::config::DiscordConfig {
                        token: "fake".to_string(),
                        guild_id: None,
                        channel_mappings: None,
                    },
                    runtime: crate::config::RuntimeConfig::default(),
                    guardian: None,
                },
            )
            .is_none()
        );
    }

    #[test]
    fn test_classify_conversational_request_routes_explicit_tool() {
        let dir = tempdir().unwrap();
        write_skill(
            dir.path(),
            "snapshot",
            r#"---
name: snapshot
tools:
  stock_quote:
    description: Quote
    shell: ./snapshot.sh
    parameters:
      type: object
---
snapshot guidance
"#,
        );

        let dispatch = classify_conversational_request(
            dir.path(),
            "用 snapshot 的 stock_quote 看一下 TSLA.US 的实时股价",
        )
        .unwrap();

        match dispatch {
            ConversationalDispatch::DirectTool { tool_name, args } => {
                assert_eq!(tool_name, "stock_quote");
                assert_eq!(args["symbol"], "TSLA.US");
            }
            _ => panic!("expected direct tool route"),
        }
    }

    #[test]
    fn test_classify_conversational_request_rejects_weather_query_without_tool() {
        let dir = tempdir().unwrap();
        let dispatch = classify_conversational_request(dir.path(), "益阳天气如何？").unwrap();

        match dispatch {
            ConversationalDispatch::UnsupportedRealtime { .. } => {}
            _ => panic!("expected unsupported realtime route"),
        }
    }
}
