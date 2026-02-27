/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/guardian.rs
 * Responsibility: The Guardian (Silent Observer). Proactive background agent for systemic health and knowledge.
 */

use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::sleep;
use crate::agent_loop::execute_tool_batch;
use crate::config::Config;
use crate::llm;
use crate::tools::{get_tool_definitions, ToolBatchState, CORE_TOOL_NAMES};
use std::fs;
pub async fn run_guardian_loop(base_path: PathBuf, config: Config) -> anyhow::Result<()> {
    println!("🛡️ The Guardian has taken its post. Vigilance is eternal.");
    
    // Initial delay to let the system settle
    sleep(Duration::from_secs(10)).await;

    loop {
        println!("🛡️ Guardian Pulse: Auditing Guild health and knowledge...");
        
        let mut retry_count = 0;
        let max_retries = 3;
        
        while retry_count < max_retries {
            match perform_guardian_pulse(&base_path, &config).await {
                Ok(_) => break,
                Err(e) => {
                    retry_count += 1;
                    if retry_count < max_retries {
                        let wait_sec = retry_count * 5;
                        eprintln!("⚠️ Guardian audit failed (Attempt {}/{}): {:?}. Retrying in {}s...", retry_count, max_retries, e, wait_sec);
                        sleep(Duration::from_secs(wait_sec)).await;
                    } else {
                        eprintln!("❌ Guardian audit failed after {} attempts: {:?}", max_retries, e);
                    }
                }
            }
        }

        // Pulse every hour (3600 seconds)
        sleep(Duration::from_secs(3600)).await;
    }

}

async fn perform_guardian_pulse(base_path: &Path, config: &Config) -> anyhow::Result<()> {
    let guardian_prompt_path = base_path.join("agents").join("GUARDIAN.md");
    let system_prompt = fs::read_to_string(guardian_prompt_path)
        .unwrap_or_else(|_| "You are the Guardian of the Guild. Monitor and maintain.".to_string());

    let tools = get_tool_definitions(base_path, config);

    // 1. Gather environmental context
    let mut env_context = String::new();
    env_context.push_str("### Environmental Context:\n");
    
    // Scan for recent history (last 5 files)
    let history_dir = base_path.join("channels").read_dir()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.path().join("history"))
        .filter(|p| p.exists())
        .collect::<Vec<_>>();
    
    env_context.push_str("- Recent Archive Folders: ");
    for h in history_dir.iter().take(3) {
        if let Ok(recent) = h.read_dir() {
            let count = recent.count();
            env_context.push_str(&format!("{:?} ({} files), ", h.parent().unwrap().file_name().unwrap(), count));
        }
    }
    env_context.push_str("\n");

    // Scan for current knowledge
    let global_knowledge_path = base_path.join("brain").join("KNOWLEDGE.md");
    let global_knowledge = fs::read_to_string(&global_knowledge_path).unwrap_or_default();
    env_context.push_str(&format!("\n### Global Knowledge Baseline:\n{}\n", global_knowledge));

    let mut messages = vec![
        llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(format!(
                "{}\n\nCore Tools: {}\n\nAvailable Tool Declarations:\n{}\n\nPerform a proactive maintenance turn. Prefer the core tools for inspection and memory maintenance. Use a discovered skill only when you need a domain-specific capability. If you find information in history that isn't distilled, use 'write' or 'edit' to update KNOWLEDGE.md. If you see anomalies, create a ritual.", 
                env_context,
                CORE_TOOL_NAMES.join(", "),
                serde_json::to_string_pretty(&tools).unwrap_or_default()
            ))]
        }
    ];

    let mut turn = 0;
    let max_turns = 3; // Guardian turns are more expensive/broader
    let guardian_blackboard = base_path.join("brain").join(".guardian-runtime.md");
    if !guardian_blackboard.exists() {
        let _ = fs::write(&guardian_blackboard, "");
    }
    let mut batch_state = ToolBatchState::default();

    while turn < max_turns {
        turn += 1;
        println!("🛡️ Guardian Turn {}/{}: Reasoning...", turn, max_turns);

        // 2. Call LLM to decide on proactive actions
        let guard_model = config.guardian.as_ref()
            .and_then(|g| g.model.as_ref())
            .unwrap_or(&config.gemini.model);

        println!("🛡️ Guardian Pulse using model: {}", guard_model);

        let turn_result = llm::generate_turn(
            &system_prompt,
            messages.clone(),
            &config.gemini.api_key,
            guard_model,
            0.5,
            Some(serde_json::json!([{ "functionDeclarations": tools }]))
        ).await?;
        match turn_result {
            llm::ModelTurn::Narrative(result) => {
                println!("🛡️ Guardian finished pulse with narrative: {}", result);
                messages.push(llm::Message {
                    role: llm::MessageRole::Assistant,
                    parts: vec![llm::MultimodalPart::text(result)],
                });
                return Ok(());
            }
            llm::ModelTurn::ToolCalls { thought, calls, parts } => {
                if let Some(thought) = thought.as_ref() {
                    println!("🛡️ Guardian Thought: {}", thought);
                }

                let assistant_parts = if parts.is_empty() {
                    thought
                        .as_ref()
                        .map(|value| vec![llm::MultimodalPart::text(format!("Thought: {}", value))])
                        .unwrap_or_default()
                } else {
                    parts
                };

                messages.push(llm::Message {
                    role: llm::MessageRole::Assistant,
                    parts: assistant_parts,
                });
                execute_tool_batch(
                    &mut messages,
                    &calls,
                    &guardian_blackboard,
                    base_path,
                    config,
                    "0",
                    &mut batch_state,
                )
                .await?;
            }
        }
    }

    Ok(())
}
