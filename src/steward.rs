/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/steward.rs
 * Responsibility: The Steward. Observe the Workspace (Channels) and fulfill the intent inscribed on Blackboards (Threads).
 */

use crate::discord;
use chrono::Local;
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::llm;
use crate::skills::{self, SkillMetadata};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tokio::sync::Semaphore;
use base64::{Engine as _, engine::general_purpose};

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct TaskHeader {
    status: String,
    schedule: Option<String>,
    injection_template: Option<String>,
    origin_channel: Option<String>,
}

static EXECUTING_FILES: Lazy<Mutex<HashSet<PathBuf>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static CONCURRENCY_LIMITER: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(5)));
pub async fn execute_thread_file(path: &PathBuf, base_path: &Path, config: &Config, trigger_id: Option<String>, target_channel_id: Option<String>, target_guild_id: Option<String>) -> anyhow::Result<()> {


    {
        let mut executing = EXECUTING_FILES.lock().unwrap();
        if executing.contains(path) {
            return Ok(());
        }
        executing.insert(path.clone());
    }

    let _permit = CONCURRENCY_LIMITER.acquire().await.unwrap();
    let res = execute_thread_file_internal(path, base_path, config, trigger_id, target_channel_id, target_guild_id).await;



    {
        let mut executing = EXECUTING_FILES.lock().unwrap();
        executing.remove(path);
    }
    res
}

async fn execute_thread_file_internal(path: &PathBuf, base_path: &Path, config: &Config, trigger_id: Option<String>, target_channel_id: Option<String>, _target_guild_id: Option<String>) -> anyhow::Result<()> {


    let mut content = fs::read_to_string(path)?;
    
    let is_log = is_conversational_log(path);
    let thread_id = path.strip_prefix(base_path.join("channels"))
        .unwrap_or(path)
        .to_str()
        .unwrap_or("unknown");

    let channel_id = match target_channel_id {
        Some(id) => id,
        None => {
            let fallback = extract_channel_id_from_path(path);
            println!("⚠️ Steward using fallback channel ID: {} for {:?}", fallback, path.file_name());
            fallback
        }
    };



    let header_owned = parse_task_document(&content).map(|(h, _)| h);
    
    if !is_log && header_owned.is_none() {
        return Ok(());
    }


    // CRITICAL: Only allowed in non-log files (e.g., Rituals)
    let re_todo = Regex::new(r"- \[ \] (.*)").unwrap();
    
    if !is_log {
        // Drain all tasks in one pass
        while let Some(caps) = re_todo.captures(&content) {
            let task_line = caps.get(0).unwrap().as_str();
            let task_desc = caps.get(1).unwrap().as_str();

            println!("⚙️ Executing step in #{}: {}", thread_id, task_desc);

            let result = match run_react_loop(task_desc, &content, path, base_path, config, &channel_id).await {
                Ok(res) => res,
                Err(e) => {
                    let err_msg = format!("Error executing task: {}", e);
                    println!("❌ {}", err_msg);
                    err_msg
                }
            };

            // Only mark as completed if no error occurred in the ReAct loop itself
            if !result.starts_with("Error executing task:") && !result.starts_with("Gemini API Error:") {
                let updated_line = task_line.replace("[ ]", "[x]");
                let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                let log_entry = format!("\n> [{}] Execution result: {}", timestamp, result);
                
                content = content.replace(task_line, &updated_line);
                content.push_str(&log_entry);
                
                // Write back immediately so other watchers can see progress
                fs::write(path, &content)?;

                let sanitized_result = mask_sensitive_data(&result, config);
                if let Err(e) = discord::send_bot_message(
                    &config.discord.token, 
                    &channel_id,
                    &format!("⚙️ Step completed in **#{}**\n{}", thread_id, sanitized_result)
                ).await {
                    eprintln!("❌ Failed to send Discord ritual message to {}: {:?}", channel_id, e);
                }


            } else {
                // Task failed, log error but keep [ ] so it can be retried
                let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                let log_entry = format!("\n> [{}] ❌ Task failed: {}", timestamp, result);
                content.push_str(&log_entry);
                fs::write(path, &content)?;
                break; // Stop draining tasks if one fails
            }
        }
    } else {
        // Conversational Mode: Triggered via MPSC or if no tasks found
        println!("🗣️ Conversational Mode in #{}...", thread_id);
        let _ = discord::broadcast_typing(&config.discord.token, &channel_id).await;
        
        match run_conversational_loop(&content, path, base_path, config, trigger_id, &channel_id).await {
            Ok(result) => {
                let sanitized_result = mask_sensitive_data(&result, config);
                match discord::send_bot_message(
                    &config.discord.token, 
                    &channel_id,
                    &sanitized_result
                ).await {
                    Ok(msg) => {
                        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                        let bot_name = &msg.author.name;
                        let bot_id = msg.author.id.to_string();
                        let msg_id = msg.id.to_string();
                        
                        let response_entry = format!(
                            "\n---\n**Author**: {} (ID: {}) | **Time**: {} | **Message ID**: {}\n\n{}\n", 
                            bot_name, bot_id, timestamp, msg_id, result
                        );
                        content.push_str(&response_entry);
                        let _ = fs::write(path, &content);
                    },
                    Err(e) => {
                        eprintln!("❌ Failed to send Discord message to {}: {:?}", channel_id, e);
                        // Fallback log without Discord Message ID
                        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                        let response_entry = format!("\n\n> [Tellar] ({}): {}\n", timestamp, result);
                        content.push_str(&response_entry);
                        let _ = fs::write(path, &content);
                    }
                }
            },

            Err(e) => {
                eprintln!("❌ Steward loop failed in #{}: {:?}", thread_id, e);
                let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                let error_entry = format!("\n\n> [Tellar] ({}): ❌ Error processing request: {}", timestamp, e);
                content.push_str(&error_entry);
                let _ = fs::write(path, &content);

                let _ = discord::send_bot_message(
                    &config.discord.token, 
                    &channel_id,
                    "⚠️ *管家在处理您的请求时遇到了异常，请稍后再试，或检查黑板记录。*"
                ).await;
            }
        }
    }



    // 6. Check for auto-archiving (Ephemeral Threads only)
    if let Some(header) = header_owned {

        if header.schedule.is_none() || header.schedule.as_ref().unwrap().is_empty() {
            let re_any_todo = Regex::new(r"- \[ \]").unwrap();
            if !re_any_todo.is_match(&content) {
                if let Some(parent) = path.parent() {
                    let today = Local::now().format("%Y-%m-%d").to_string();
                    let history_dir = parent.join("history").join(&today);
                    let _ = fs::create_dir_all(&history_dir);
                    
                    if let Some(file_name) = path.file_name() {
                        let dest_path = history_dir.join(file_name);
                        if let Err(e) = fs::rename(path, &dest_path) {
                            eprintln!("⚠️ Failed to archive thread: {:?}", e);
                        } else {
                            println!("📦 Thread archived to history/{}", today);
                            let _ = discord::send_bot_message(
                                &config.discord.token,
                                &channel_id,
                                &format!("📦 Thread **#{}** has been archived to history/{}", thread_id, today)
                            ).await;

                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// The Core Generic Agent Loop (pi-mono style)
pub(crate) async fn run_agent_loop(
    initial_messages: Vec<llm::Message>,
    path: &Path,
    base_path: &Path,
    config: &Config,
    channel_id: &str,
    system_prompt: &str,
) -> anyhow::Result<String> {
    let tools = get_tool_definitions(base_path);
    let mut messages = initial_messages;
    let max_turns = 50;
    let mut turn = 0;

    while turn < max_turns {
        turn += 1;
        println!("🧠 Turn {}/{}: Reasoning...", turn, max_turns);

        // 🟢 Steering: Reload blackboard to see if user interrupted us
        update_history_with_steering(&mut messages, path).await?;

        let result = llm::generate_multimodal(
            system_prompt,
            messages.clone(),
            &config.gemini.api_key,
            &config.gemini.model,
            0.5,
            Some(json!([{ "functionDeclarations": tools }]))
        ).await?;

        // Parse thought/tool/finish
        let tool_call: Value = match parse_llm_json(&result) {
            Some(v) => v,
            None => {
                println!("⚠️ LLM output was not JSON, assuming final answer.");
                return Ok(result);
            }
        };

        // Standardize Thought
        let mut assistant_parts = Vec::new();
        if let Some(thought) = tool_call["thought"].as_str() {
            println!("💬 Thought: {}", thought);
            assistant_parts.push(llm::MultimodalPart::text(format!("Thought: {}", thought)));
        }

        // Handle Finish
        if let Some(finish_msg) = tool_call["finish"].as_str() {
            println!("✅ Task completed: {}", finish_msg);
            return Ok(finish_msg.to_string());
        }

        // Handle Tool Call
        if let Some(tool_name) = tool_call["tool"].as_str() {
            let default_args = json!({});
            let args = tool_call.get("args").unwrap_or(&default_args);
            let thought_signature = tool_call["thought_signature"].as_str().map(|s| s.to_string());
            println!("🛠️ Action: `{}`", tool_name);

            // Record the tool call in history
            assistant_parts.push(llm::MultimodalPart::function_call(tool_name, args.clone(), thought_signature));
            messages.push(llm::Message {
                role: llm::MessageRole::Assistant,
                parts: assistant_parts,
            });

            // Execute
            let observation = dispatch_tool(tool_name, args, base_path, config, channel_id).await;
            println!("👁️ Observation: [{} characters]", observation.len());

            // Record result in history
            messages.push(llm::Message {
                role: llm::MessageRole::ToolResult,
                parts: vec![llm::MultimodalPart::function_response(tool_name, json!({ "output": observation }))],
            });

            // 🟢 Mid-turn Steering: Check if user sent a message precisely WHILE tool was running
            update_history_with_steering(&mut messages, path).await?;
        } else {
            // No tool, No finish, just text
            messages.push(llm::Message {
                role: llm::MessageRole::Assistant,
                parts: vec![llm::MultimodalPart::text(result.clone())],
            });
            return Ok(result);
        }
    }

    Ok("Max iterations reached.".to_string())
}

/// Reread the blackboard and inject any NEW messages into the history
async fn update_history_with_steering(messages: &mut Vec<llm::Message>, path: &Path) -> anyhow::Result<()> {
    let current_content = fs::read_to_string(path).unwrap_or_default();
    
    // Simple heuristic: if the total message count in blackboard appears to have grown
    // compared to what we have in history, try to find the new chunks.
    // In a production Tellar, we'd use message IDs, but for now we look for new "Author" blocks
    // that aren't already represented in our 'User' role messages.
    
    // For simplicity in this refactor, we extract all user messages from blackboard 
    // and see if the last one is DIFFERENT from our last recorded User message.
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
    let last_history_msg = messages.iter().rev()
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

pub(super) async fn run_react_loop(
    task: &str,
    full_context: &str,
    path: &Path,
    base_path: &Path,
    config: &Config,
    channel_id: &str,
) -> anyhow::Result<String> {
    let mut system_prompt_str = load_unified_prompt(base_path, channel_id);
    
    let mut channel_memory = String::new();
    // Check for origin_channel binding
    if let Some((header, _)) = parse_task_document(full_context) {
        if let Some(origin_id) = header.origin_channel {
            // Robust Resolution: Always resolve folder by ID suffix anchor
            if let Some(robust_folder) = discord::resolve_folder_by_id(base_path, &origin_id) {
                let knowledge_path = base_path.join("channels").join(&robust_folder).join("KNOWLEDGE.md");
                if knowledge_path.exists() {
                    println!("🧠 Ritual linked to current channel folder: #{} (ID: {}), loading knowledge...", robust_folder, origin_id);
                    channel_memory = fs::read_to_string(knowledge_path).unwrap_or_default();
                }
            } else {
                // Fallback: If no channel folder found (e.g. deleted), load from 'general' or skip
                println!("⚠️ Ritual origin channel (ID: {}) not found locally, skipping channel-specific knowledge.", origin_id);
            }
        }
    }

    // Also load local ritual knowledge if any
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

    system_prompt_str.push_str(&format!("\n\n### Semantic Memory (Channel):\n{}\n\n### Semantic Memory (Global):\n{}", channel_memory, global_memory));

    let mut initial_messages = vec![
        llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(format!(
                "### Current Blackboard Context:\n{}\n\n### Your Objective:\nYou are currently processing the step: \"{}\".\nExecute valid tool calls to satisfy this step. Use 'finish' if you are done.",
                full_context, task
            ))]
        }
    ];

    // Vision
    let mut image_parts = extract_and_load_images(full_context, base_path);
    if !image_parts.is_empty() {
        initial_messages[0].parts.append(&mut image_parts);
    }

    run_agent_loop(initial_messages, path, base_path, config, channel_id, &system_prompt_str).await
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
    system_prompt_str.push_str(&format!("\n\n### Semantic Memory (Channel Knowledge):\n{}", channel_memory));

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
    
    let mut initial_messages = vec![
        llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(format!(
                "### Current User Input (Specific Target):\n{}\n\nRespond naturally. Use Markdown. Concise yet premium.",
                trigger_instruction
            ))]
        }
    ];

    let mut image_parts = extract_and_load_images(full_context, base_path);
    if !image_parts.is_empty() {
        initial_messages[0].parts.append(&mut image_parts);
    }

    run_agent_loop(initial_messages, path, base_path, config, channel_id, &system_prompt_str).await
}

/// Loads the unified system prompt: Base AGENTS.md + optional <CHANNEL_ID>.AGENTS.md
fn load_unified_prompt(base_path: &Path, channel_id: &str) -> String {
    let agents_dir = base_path.join("agents");
    let base_prompt_path = agents_dir.join("AGENTS.md");
    
    let mut system_prompt = fs::read_to_string(base_prompt_path)
        .unwrap_or_else(|_| "You are Tellar, a cyber steward.".to_string());

    // Load channel-specific override if it exists
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

/// Helper to parse structured JSON from LLM response, handling markdown blocks
pub fn parse_llm_json(result: &str) -> Option<Value> {
    let clean_result = if result.contains("```json") {
        result.split("```json").nth(1).unwrap_or(result).split("```").next().unwrap_or(result).trim()
    } else if result.contains("```") {
        result.split("```").nth(1).unwrap_or(result).split("```").next().unwrap_or(result).trim()
    } else {
        result.trim()
    };

    serde_json::from_str(clean_result).ok()
}


fn extract_and_load_images(text: &str, base_path: &Path) -> Vec<llm::MultimodalPart> {
    let mut parts = Vec::new();
    // Pattern: (local: [file://path/to/image.png])
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

fn normalize_path(path: &str) -> &str {
    let p = path.strip_prefix("guild/").unwrap_or(path);
    p.strip_prefix("./").unwrap_or(p)
}

fn is_path_safe(_base: &Path, rel: &str) -> bool {
    // canonicalize to resolve .. and symlinks, then check prefix
    // Note: canonicalize requires the path to exist, which might not be true for 'write'.
    // A simpler check for '..' is more robust for new files.

    if rel.contains("..") || rel.starts_with("/") {
        return false;
    }
    true
}

/// Masks sensitive tokens and keys in the output string to prevent privacy leaks.
pub fn mask_sensitive_data(text: &str, config: &Config) -> String {
    let mut masked = text.to_string();
    
    // Mask Gemini API Key
    if !config.gemini.api_key.is_empty() && config.gemini.api_key.len() > 10 {
        masked = masked.replace(&config.gemini.api_key, "[REDACTED_GEMINI_KEY]");
    }
    
    // Mask Discord Bot Token
    if !config.discord.token.is_empty() && config.discord.token.len() > 10 {
        masked = masked.replace(&config.discord.token, "[REDACTED_DISCORD_TOKEN]");
    }
    
    masked
}



pub(crate) async fn dispatch_tool(name: &str, args: &Value, base_path: &Path, config: &Config, channel_id: &str) -> String {
    let output = match name {
        "upload" => {
            let rel_path = normalize_path(args["path"].as_str().unwrap_or(""));
            if !is_path_safe(base_path, rel_path) {
                return "Error: Access denied. Path must be within the guild directory.".to_string();
            }
            let full_path = base_path.join(rel_path);
            match discord::send_file_attachment(&config.discord.token, channel_id, &full_path).await {
                Ok(_) => format!("Successfully uploaded {} to Discord.", rel_path),
                Err(e) => format!("Error uploading file: {}", e),
            }
        },
        "sh" => {
            let cmd_str = args["command"].as_str().unwrap_or("");
            if cmd_str.is_empty() { "Error: No command provided".into() } else {
                let child = Command::new("sh")
                    .current_dir(base_path) // Enforce guild-scoped execution
                    .arg("-c")
                    .arg(cmd_str)
                    .output();
                
                match timeout(Duration::from_secs(30), child).await {
                    Ok(res) => match res {
                        Ok(out) => {
                            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                            format!("STDOUT:\n{}\nSTDERR:\n{}", stdout, stderr)
                        },
                        Err(e) => format!("Error executing sh: {}", e),
                    },
                    Err(_) => "Error: Command timed out after 30 seconds. Please try a more targeted search or specific directory (avoid `find /`).".to_string(),
                }
            }
        },
        "read" => {
            let rel_path = normalize_path(args["path"].as_str().unwrap_or(""));
            if !is_path_safe(base_path, rel_path) {
                return "Error: Access denied. Path must be within the guild directory.".to_string();
            }

            let offset = args["offset"].as_u64().unwrap_or(1) as usize; // 1-indexed
            let limit = args["limit"].as_u64().unwrap_or(800) as usize;

            let file_path = base_path.join(rel_path);
            if !file_path.exists() { format!("Error: File not found: {}", rel_path) } else {
                match fs::read_to_string(&file_path) {
                    Ok(content) => {
                        let lines: Vec<&str> = content.lines().collect();
                        if offset > lines.len() {
                            format!("Error: offset {} is beyond file length {}", offset, lines.len())
                        } else {
                            let end = std::cmp::min(offset - 1 + limit, lines.len());
                            lines[(offset - 1)..end].join("\n")
                        }
                    },
                    Err(e) => format!("Error reading file: {}", e),
                }
            }
        },
        "write" => {
            let rel_path = normalize_path(args["path"].as_str().unwrap_or(""));
            if !is_path_safe(base_path, rel_path) {
                return "Error: Access denied. Path must be within the guild directory.".to_string();
            }

            let content = args["content"].as_str().unwrap_or("");
            let full_path = base_path.join(rel_path);
            
            if let Some(parent) = full_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            
            match fs::write(&full_path, content) {
                Ok(_) => format!("Successfully wrote to {}", rel_path),
                Err(e) => format!("Error writing file: {}", e),
            }
        },
        "edit" => {
            let rel_path = normalize_path(args["path"].as_str().unwrap_or(""));
            if !is_path_safe(base_path, rel_path) {
                return "Error: Access denied. Path must be within the guild directory.".to_string();
            }

            let old_text = args["oldText"].as_str().unwrap_or("");
            let new_text = args["newText"].as_str().unwrap_or("");
            
            match fs::read_to_string(base_path.join(rel_path)) {
                Ok(content) => {
                    let occurrences: Vec<_> = content.matches(old_text).collect();
                    if occurrences.len() == 1 {
                        let new_content = content.replace(old_text, new_text);
                        match fs::write(base_path.join(rel_path), new_content) {
                            Ok(_) => format!("Successfully edited {}", rel_path),
                            Err(e) => format!("Error writing file: {}", e),
                        }
                    } else if occurrences.is_empty() {
                        format!("Error: oldText not found in {}", rel_path)
                    } else {
                        format!("Error: oldText is not unique in {} (found {} occurrences)", rel_path, occurrences.len())
                    }
                },
                Err(_e) => format!("Error: File not found: {}", rel_path),
            }
        },

        _ => {
            let skills = SkillMetadata::discover_skills(base_path);
            let mut skill_out = format!("Error: Unknown tool `{}`", name);
            for (meta, dir) in skills {
                if let Some(tool) = meta.tools.get(name) {
                    skill_out = match skills::execute_skill_tool(&tool.shell, &dir, args, config).await {
                        Ok(out) => out,
                        Err(e) => format!("Error executing skill tool `{}`: {}", name, e),
                    };
                    break;
                }
            }
            skill_out
        }
    };

    // Safety Guard: Truncate massive outputs to prevent LLM/API crashes
    truncate_output(output)
}


fn truncate_output(output: String) -> String {
    let limit = 5000;
    if output.len() > limit {
        // Find safe UTF-8 boundaries
        let mut prefix_end = 2500;
        while prefix_end > 0 && !output.is_char_boundary(prefix_end) {
            prefix_end -= 1;
        }
        
        let mut suffix_start = output.len() - 2500;
        while suffix_start < output.len() && !output.is_char_boundary(suffix_start) {
            suffix_start += 1;
        }

        let prefix = &output[..prefix_end];
        let suffix = &output[suffix_start..];
        
        format!(
            "{} ... [TRUNCATED {} bytes] ... {}\n\n💡 **Hint**: Data is too large for the session history. If you need the full original content or the entire file, please use the `upload` tool instead of displaying it here.",
            prefix,
            output.len() - (prefix_end + (output.len() - suffix_start)),
            suffix
        )
    } else {
        output
    }
}




pub(crate) fn get_tool_definitions(base_path: &Path) -> Value {
    let mut tools = json!([
        {
            "name": "read",
            "description": "Read the contents of a file. Supports line-based reading with offset and limit.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to read (relative to guild root)" },
                    "offset": { "type": "number", "description": "Line number to start reading from (1-indexed)" },
                    "limit": { "type": "number", "description": "Maximum number of lines to read" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "write",
            "description": "Write content to a file. Overwrites existing content. Creates parent directories.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to write (relative to guild root)" },
                    "content": { "type": "string", "description": "The content to write" }
                },
                "required": ["path", "content"]
            }
        },
        {
            "name": "edit",
            "description": "Precision surgical edit. Replaces an exact string with a new one. Fails if the old string is not unique or not found.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to edit" },
                    "oldText": { "type": "string", "description": "The EXACT text to find and replace" },
                    "newText": { "type": "string", "description": "The new text to replace it with" }
                },
                "required": ["path", "oldText", "newText"]
            }
        },
        {
            "name": "sh",
            "description": "Execute a shell command. Best for system discovery, grep, process status, and small output observation. For large data or sending files to the user, the `upload` tool is much more efficient.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The shell command to execute" }
                },
                "required": ["command"]
            }
        },
        {
            "name": "upload",
            "description": "Upload a file from the local system to the current Discord channel as an attachment. Use this for large files or when the user specifically asks for the file.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to the file within the guild (e.g. 'channels/General/data.txt') or absolute path if authorized." }
                },
                "required": ["path"]
            }
        }
    ]);

    let discovered = SkillMetadata::discover_skills(base_path);
    for (meta, _) in discovered {
        for (tool_name, tool_info) in meta.tools {
            tools.as_array_mut().unwrap().push(json!({
                "name": tool_name,
                "description": format!("{}: {}", meta.name, tool_info.description),
                "parameters": tool_info.parameters
            }));
        }
    }
    tools
}

fn parse_task_document(content: &str) -> Option<(TaskHeader, &str)> {
    if !content.starts_with("---") { return None; }
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 { return None; }
    let yaml_str = parts[1];
    let body = parts[2].trim();
    if let Ok(header) = serde_yaml::from_str::<TaskHeader>(yaml_str) {
        Some((header, body))
    } else {
        None
    }
}

fn is_conversational_log(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    // Matches YYYY-MM-DD.md
    file_name.len() == 13 && 
    file_name.chars().nth(4) == Some('-') && 
    file_name.chars().nth(7) == Some('-') &&
    path.extension().and_then(|s| s.to_str()) == Some("md")
}

fn extract_channel_id_from_path(path: &Path) -> String {
    // 1. Try to read from YAML header first
    if let Ok(content) = fs::read_to_string(path) {
        if let Some((header, _)) = parse_task_document(&content) {
            if let Some(origin) = header.origin_channel {
                if origin != "0" { return origin; }
            }
        }
    }

    // 2. Fallback: Parse from parent folder name
    if let Some(parent) = path.parent() {
        if let Some(folder_name) = parent.file_name().and_then(|s| s.to_str()) {
            if let Some(id) = discord::extract_id_from_folder(folder_name) {
                return id;
            }
        }
    }

    "0".to_string()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_steering_detection() -> anyhow::Result<()> {
        let path = std::env::current_dir()?.join("test_blackboard.md");
        
        // 1. Initial State
        let content = "**Author**: User1 | **Time**: 12:00\n\nHello\n";
        fs::write(&path, content)?;

        let mut messages = vec![
            llm::Message {
                role: llm::MessageRole::User,
                parts: vec![llm::MultimodalPart::text("Hello".to_string())],
            }
        ];

        // Should NOT detect anything new
        update_history_with_steering(&mut messages, &path).await?;
        assert_eq!(messages.len(), 1);

        // 2. User interrupts!
        let new_content = "**Author**: User1 | **Time**: 12:00\n\nHello\n\n---\n**Author**: User1 | **Time**: 12:01\n\nSTOP!\n";
        fs::write(&path, new_content)?;

        update_history_with_steering(&mut messages, &path).await?;
        
        // Should detect "STOP!"
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, llm::MessageRole::User);
        assert_eq!(messages[1].parts[0].text.as_ref().unwrap(), "STOP!");

        // Cleanup
        let _ = fs::remove_file(&path);
        
        Ok(())
    }
}
