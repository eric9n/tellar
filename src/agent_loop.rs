/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/agent_loop.rs
 * Responsibility: Native tool-calling agent loop and batch execution policy.
 */

use crate::config::Config;
use crate::context::update_history_with_steering;
use crate::llm;
use crate::tools::{
    ToolBatchState, ToolExecutionResult, dispatch_tool, get_tool_definitions, is_read_only_tool,
    is_write_tool, push_system_note, push_tool_result_message, skip_remaining_tool_calls,
    tool_call_signature, tool_observation_signature,
};
use serde_json::json;
use std::path::Path;

/// The core agent loop for native tool-calling turns.
pub(crate) async fn run_agent_loop(
    initial_messages: Vec<llm::Message>,
    path: &Path,
    base_path: &Path,
    config: &Config,
    _channel_id: &str,
    system_prompt: &str,
) -> anyhow::Result<String> {
    let tools = get_tool_definitions(base_path);
    let mut messages = initial_messages;
    let max_turns = config.runtime.max_turns.max(1);
    let mut turn = 0;
    let mut batch_state = ToolBatchState::default();

    while turn < max_turns {
        turn += 1;
        println!("🧠 Turn {}/{}: Reasoning...", turn, max_turns);

        update_history_with_steering(&mut messages, path).await?;

        let turn_result = llm::generate_turn(
            system_prompt,
            messages.clone(),
            &config.gemini.api_key,
            &config.gemini.model,
            0.5,
            Some(json!([{ "functionDeclarations": tools }])),
        )
        .await?;
        match turn_result {
            llm::ModelTurn::Narrative(result) => {
                messages.push(llm::Message {
                    role: llm::MessageRole::Assistant,
                    parts: vec![llm::MultimodalPart::text(result.clone())],
                });
                return Ok(result);
            }
            llm::ModelTurn::ToolCalls { thought, calls, parts } => {
                if let Some(thought) = thought.as_ref() {
                    println!("💬 Thought: {}", thought);
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
                execute_tool_batch(&mut messages, &calls, path, base_path, config, &mut batch_state)
                    .await?;
            }
        }
    }

    Ok("Max iterations reached.".to_string())
}

pub(crate) async fn execute_tool_batch(
    messages: &mut Vec<llm::Message>,
    calls: &[llm::ToolCallRequest],
    path: &Path,
    base_path: &Path,
    config: &Config,
    batch_state: &mut ToolBatchState,
) -> anyhow::Result<()> {
    // Read-only tools can batch within a turn; state-mutating tools must force a new turn.
    let read_only_budget = config.runtime.read_only_budget.max(1);
    let mut read_only_calls = 0;
    let mut stop_reason: Option<String> = None;

    for (index, call) in calls.iter().enumerate() {
        let call_signature = tool_call_signature(call);
        if batch_state.last_call_signature.as_ref() == Some(&call_signature) {
            let observation = ToolExecutionResult::error(
                "Skipped repeated tool call with unchanged arguments. Change strategy.",
            );
            println!("⚠️ Skipping repeated action: `{}`", call.name);
            push_tool_result_message(messages, call, &observation);
            batch_state.no_new_info_streak += 1;
            batch_state.repeated_error_streak += 1;
            stop_reason = Some("Repeated tool call detected.".to_string());
            skip_remaining_tool_calls(
                messages,
                calls,
                index + 1,
                "Skipped because the current batch was cut short and the model must reevaluate.",
            );
            break;
        }

        println!("🛠️ Action: `{}`", call.name);
        let observation = dispatch_tool(&call.name, &call.args, base_path, config).await;
        println!("👁️ Observation: [{} characters]", observation.output.len());
        push_tool_result_message(messages, call, &observation);

        let observation_signature = tool_observation_signature(&observation);
        if batch_state.last_observation_signature.as_ref() == Some(&observation_signature) {
            batch_state.no_new_info_streak += 1;
        } else {
            batch_state.no_new_info_streak = 0;
        }

        if observation.is_error
            && batch_state.last_observation_signature.as_ref() == Some(&observation_signature)
        {
            batch_state.repeated_error_streak += 1;
        } else if observation.is_error {
            batch_state.repeated_error_streak = 1;
        } else {
            batch_state.repeated_error_streak = 0;
        }

        batch_state.last_call_signature = Some(call_signature);
        batch_state.last_observation_signature = Some(observation_signature);

        if is_read_only_tool(&call.name) {
            read_only_calls += 1;
            if read_only_calls >= read_only_budget {
                stop_reason = Some(format!(
                    "Read-only budget reached ({} calls). Reevaluate before continuing.",
                    read_only_budget
                ));
                skip_remaining_tool_calls(
                    messages,
                    calls,
                    index + 1,
                    "Skipped because the read-only budget for this turn was reached.",
                );
                break;
            }
        }

        if is_write_tool(&call.name) {
            stop_reason = Some(format!(
                "State changed via `{}`. Reevaluate before more actions.",
                call.name
            ));
            skip_remaining_tool_calls(
                messages,
                calls,
                index + 1,
                "Skipped because workspace state changed and the model must reevaluate.",
            );
            break;
        }

        if batch_state.repeated_error_streak >= 2 {
            stop_reason =
                Some("Repeated similar tool errors detected. Change strategy or finish.".to_string());
            skip_remaining_tool_calls(
                messages,
                calls,
                index + 1,
                "Skipped because repeated similar tool errors require a new strategy.",
            );
            break;
        }

        if batch_state.no_new_info_streak >= 2 {
            stop_reason = Some(
                "Recent tool calls are not producing new information. Change strategy or finish."
                    .to_string(),
            );
            skip_remaining_tool_calls(
                messages,
                calls,
                index + 1,
                "Skipped because recent tool calls were not producing new information.",
            );
            break;
        }

        let before_len = messages.len();
        update_history_with_steering(messages, path).await?;
        if messages.len() > before_len {
            stop_reason = Some("A new user message arrived. Reevaluate before more actions.".to_string());
            skip_remaining_tool_calls(
                messages,
                calls,
                index + 1,
                "Skipped because a new user message arrived and the model must reevaluate.",
            );
            break;
        }
    }

    if let Some(reason) = stop_reason {
        push_system_note(messages, reason);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_config() -> Config {
        Config {
            gemini: crate::config::GeminiConfig {
                api_key: "fake".to_string(),
                model: "fake-model".to_string(),
            },
            discord: crate::config::DiscordConfig {
                token: "fake".to_string(),
                guild_id: None,
                channel_mappings: None,
            },
            runtime: crate::config::RuntimeConfig::default(),
            guardian: None,
        }
    }

    #[tokio::test]
    async fn test_execute_tool_batch_stops_after_write() {
        let dir = tempdir().unwrap();
        let blackboard = dir.path().join("blackboard.md");
        std::fs::write(&blackboard, "").unwrap();
        let mut messages = Vec::new();
        let calls = vec![
            llm::ToolCallRequest {
                id: "call-1".to_string(),
                name: "write".to_string(),
                args: json!({ "path": "note.txt", "content": "hello" }),
            },
            llm::ToolCallRequest {
                id: "call-2".to_string(),
                name: "read".to_string(),
                args: json!({ "path": "note.txt" }),
            },
        ];

        execute_tool_batch(
            &mut messages,
            &calls,
            &blackboard,
            dir.path(),
            &test_config(),
            &mut ToolBatchState::default(),
        )
        .await
        .unwrap();

        let tool_results = messages
            .iter()
            .filter(|m| matches!(m.role, llm::MessageRole::ToolResult))
            .count();
        assert_eq!(tool_results, 2);
        assert!(dir.path().join("note.txt").exists());
        let final_note = messages.last().unwrap().parts[0].text.as_ref().unwrap();
        assert!(final_note.contains("State changed via `write`"));
    }

    #[tokio::test]
    async fn test_execute_tool_batch_skips_repeated_call() {
        let dir = tempdir().unwrap();
        let blackboard = dir.path().join("blackboard.md");
        std::fs::write(&blackboard, "").unwrap();
        let mut messages = Vec::new();
        let calls = vec![
            llm::ToolCallRequest {
                id: "call-1".to_string(),
                name: "find".to_string(),
                args: json!({ "name": "alpha" }),
            },
            llm::ToolCallRequest {
                id: "call-2".to_string(),
                name: "ls".to_string(),
                args: json!({ "path": "." }),
            },
        ];
        let mut batch_state = ToolBatchState::default();
        batch_state.last_call_signature = Some(tool_call_signature(&calls[0]));

        execute_tool_batch(
            &mut messages,
            &calls,
            &blackboard,
            dir.path(),
            &test_config(),
            &mut batch_state,
        )
        .await
        .unwrap();

        let tool_results = messages
            .iter()
            .filter(|m| matches!(m.role, llm::MessageRole::ToolResult))
            .count();
        assert_eq!(tool_results, 2);
        let final_note = messages.last().unwrap().parts[0].text.as_ref().unwrap();
        assert!(final_note.contains("Repeated tool call detected"));
    }

    #[tokio::test]
    async fn test_execute_tool_batch_enforces_read_only_budget() {
        let dir = tempdir().unwrap();
        let blackboard = dir.path().join("blackboard.md");
        std::fs::write(&blackboard, "").unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs").join("alpha.txt"), "alpha\nfind me\n").unwrap();
        std::fs::write(dir.path().join("docs").join("beta.txt"), "beta\nother\n").unwrap();
        let mut messages = Vec::new();
        let calls = vec![
            llm::ToolCallRequest {
                id: "call-1".to_string(),
                name: "find".to_string(),
                args: json!({ "name": "alpha", "path": "docs" }),
            },
            llm::ToolCallRequest {
                id: "call-2".to_string(),
                name: "ls".to_string(),
                args: json!({ "path": "docs" }),
            },
            llm::ToolCallRequest {
                id: "call-3".to_string(),
                name: "grep".to_string(),
                args: json!({ "pattern": "find me", "path": "docs" }),
            },
            llm::ToolCallRequest {
                id: "call-4".to_string(),
                name: "read".to_string(),
                args: json!({ "path": "docs/alpha.txt" }),
            },
            llm::ToolCallRequest {
                id: "call-5".to_string(),
                name: "find".to_string(),
                args: json!({ "name": "beta", "path": "docs" }),
            },
        ];

        execute_tool_batch(
            &mut messages,
            &calls,
            &blackboard,
            dir.path(),
            &test_config(),
            &mut ToolBatchState::default(),
        )
        .await
        .unwrap();

        let tool_results = messages
            .iter()
            .filter(|m| matches!(m.role, llm::MessageRole::ToolResult))
            .count();
        assert_eq!(tool_results, 5);
        let final_note = messages.last().unwrap().parts[0].text.as_ref().unwrap();
        assert!(final_note.contains("Read-only budget reached"));
    }
}
