/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/plan_executor.rs
 * Responsibility: Execute finite conversational plans without falling into free exploration.
 */

use crate::config::Config;
use crate::llm;
use crate::router::{PlanStep, RequestRoute};
use crate::tools::dispatch_tool;
use anyhow::Result;
use std::path::Path;

pub(crate) async fn execute_conversational_route(
    dispatch: RequestRoute,
    base_path: &Path,
    config: &Config,
    channel_id: &str,
    system_prompt: &str,
    user_text: &str,
) -> Result<String> {
    match dispatch {
        RequestRoute::Agent => Ok(
            "I couldn't build a controlled execution plan for that request, so it needs the general agent path instead."
                .to_string(),
        ),
        RequestRoute::Reject { reason } => Ok(format!(
            "I can't answer that reliably right now because I don't have a matching live data tool for that request. {}",
            reason
        )),
        RequestRoute::PlanAndExecute { steps } => execute_plan_steps(
            steps,
            base_path,
            config,
            channel_id,
            system_prompt,
            user_text,
        )
        .await,
    }
}

async fn execute_plan_steps(
    steps: Vec<PlanStep>,
    base_path: &Path,
    config: &Config,
    channel_id: &str,
    system_prompt: &str,
    user_text: &str,
) -> Result<String> {
    let mut last_output: Option<String> = None;

    for step in steps {
        match step {
            PlanStep::CallTool { tool_name, args } => {
                let result = dispatch_tool(&tool_name, &args, base_path, config, channel_id).await;
                if result.is_error {
                    return Ok(format!(
                        "I tried `{}` but it failed:\n{}",
                        tool_name, result.output
                    ));
                }
                last_output = Some(result.output);
            }
            PlanStep::Respond { instruction } => {
                let observation = last_output.clone().unwrap_or_default();
                let response_prompt = format!(
                    "### Original User Request\n{}\n\n### Tool Result\n{}\n\n### Your Task\n{}",
                    user_text, observation, instruction
                );

                match llm::generate_turn(
                    system_prompt,
                    vec![llm::Message {
                        role: llm::MessageRole::User,
                        parts: vec![llm::MultimodalPart::text(response_prompt)],
                    }],
                    &config.gemini.api_key,
                    &config.gemini.model,
                    0.4,
                    None,
                )
                .await?
                {
                    llm::ModelTurn::Narrative(result) => return Ok(result),
                    llm::ModelTurn::ToolCalls { .. } => {
                        return Ok(last_output.unwrap_or_else(|| {
                            "I completed the requested tool call, but I could not finish the follow-up explanation without falling back into tool use.".to_string()
                        }));
                    }
                }
            }
            PlanStep::AskForMissing { prompt } => return Ok(prompt),
        }
    }

    Ok(last_output.unwrap_or_default())
}
