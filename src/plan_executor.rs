/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/plan_executor.rs
 * Responsibility: Execute finite conversational plans without falling into free exploration.
 */

use crate::config::Config;
use crate::execution_contract::{
    ExecutableRoute, ExecutionFinalState, ExecutionOutcome, ExecutionPlan, ExecutionStepKind,
    ExecutionStepTrace, ExecutionTrace, PlanConfidence, PlanIntent, PlanStep, ResponseStyle,
    ToolCallSpec,
};
use crate::input::Workset;
use crate::llm;
use crate::task_response::{
    append_result_confidence_notice, ask_for_missing_response, reject_route_response,
    respond_step_fallback, tool_failure_response,
};
use crate::tools::dispatch_tool;
use anyhow::Result;
use std::path::Path;

#[derive(Debug)]
struct PlanExecutionTrace {
    trace: ExecutionTrace,
}

impl PlanExecutionTrace {
    fn new(intent: PlanIntent, confidence: PlanConfidence) -> Self {
        Self {
            trace: ExecutionTrace {
                intent,
                confidence,
                steps: Vec::new(),
            },
        }
    }

    fn push(&mut self, step: ExecutionStepKind) {
        self.trace.steps.push(ExecutionStepTrace {
            index: self.trace.steps.len() + 1,
            step,
        });
    }

    fn finish(self, final_state: ExecutionFinalState, user_response: String) -> ExecutionOutcome {
        let confidence = self.trace.confidence;
        ExecutionOutcome {
            final_state,
            user_response: append_result_confidence_notice(user_response, final_state, confidence),
            trace: self.trace,
        }
    }

    fn finish_with_step(
        mut self,
        step: ExecutionStepKind,
        final_state: ExecutionFinalState,
        user_response: String,
    ) -> ExecutionOutcome {
        self.push(step);
        self.finish(final_state, user_response)
    }
}

#[derive(Debug)]
struct PlanExecutionState {
    trace: PlanExecutionTrace,
    last_output: Option<String>,
}

impl PlanExecutionState {
    fn new(intent: PlanIntent, confidence: PlanConfidence) -> Self {
        Self {
            trace: PlanExecutionTrace::new(intent, confidence),
            last_output: None,
        }
    }

    fn last_output(&self) -> Option<String> {
        self.last_output.clone()
    }

    fn apply_continue(&mut self, step: ExecutionStepKind, output: String) {
        self.trace.push(step);
        self.last_output = Some(output);
    }

    fn finish_with_step(
        self,
        step: ExecutionStepKind,
        final_state: ExecutionFinalState,
        user_response: String,
    ) -> ExecutionOutcome {
        self.trace
            .finish_with_step(step, final_state, user_response)
    }

    fn finish_with_last_output(self) -> ExecutionOutcome {
        self.trace.finish(
            ExecutionFinalState::Completed,
            self.last_output.unwrap_or_default(),
        )
    }
}

pub(crate) struct PlanExecutionContext<'a> {
    pub(crate) workset: &'a Workset,
    pub(crate) base_path: &'a Path,
    pub(crate) config: &'a Config,
    pub(crate) channel_id: &'a str,
    pub(crate) system_prompt: &'a str,
}

enum StepExecutionDirective {
    Continue {
        step: ExecutionStepKind,
        output: String,
    },
    Finish {
        step: ExecutionStepKind,
        final_state: ExecutionFinalState,
        user_response: String,
    },
}

async fn execute_call_tool_step(
    call: ToolCallSpec,
    ctx: &PlanExecutionContext<'_>,
) -> StepExecutionDirective {
    let tool_name = call.tool_name.clone();
    let result = dispatch_tool(
        &tool_name,
        &call.args,
        ctx.base_path,
        ctx.config,
        ctx.channel_id,
    )
    .await;

    if result.is_error {
        return StepExecutionDirective::Finish {
            step: ExecutionStepKind::CalledTool {
                tool_name: tool_name.clone(),
                succeeded: false,
            },
            final_state: ExecutionFinalState::Failed,
            user_response: tool_failure_response(&tool_name, &result.output),
        };
    }

    StepExecutionDirective::Continue {
        step: ExecutionStepKind::CalledTool {
            tool_name,
            succeeded: true,
        },
        output: result.output,
    }
}

fn build_respond_prompt(
    user_text: &str,
    observation: &str,
    style: ResponseStyle,
    guidance: Option<String>,
) -> String {
    format!(
        "### Original User Request\n{}\n\n### Tool Result\n{}\n\n### Response Style\n{}\n\n### Extra Guidance\n{}",
        user_text,
        observation,
        style.instruction(),
        guidance.unwrap_or_default()
    )
}

async fn execute_respond_step(
    style: ResponseStyle,
    guidance: Option<String>,
    user_text: &str,
    last_output: Option<String>,
    ctx: &PlanExecutionContext<'_>,
) -> Result<StepExecutionDirective> {
    let observation = last_output.clone().unwrap_or_default();
    let response_prompt = build_respond_prompt(user_text, &observation, style, guidance);

    match llm::generate_turn(
        ctx.system_prompt,
        vec![llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(response_prompt)],
        }],
        &ctx.config.gemini.api_key,
        &ctx.config.gemini.model,
        0.4,
        None,
    )
    .await?
    {
        llm::ModelTurn::Narrative(result) => Ok(StepExecutionDirective::Finish {
            step: ExecutionStepKind::Responded { style },
            final_state: ExecutionFinalState::Completed,
            user_response: result,
        }),
        llm::ModelTurn::ToolCalls { .. } => Ok(StepExecutionDirective::Finish {
            step: ExecutionStepKind::RespondFallback { style },
            final_state: ExecutionFinalState::Completed,
            user_response: respond_step_fallback(last_output),
        }),
    }
}

fn execute_ask_for_missing_step(
    fields: Vec<String>,
    prompt: Option<String>,
) -> StepExecutionDirective {
    StepExecutionDirective::Finish {
        step: ExecutionStepKind::RequestedMissingInput {
            prompt_only: fields.is_empty(),
            fields: fields.clone(),
        },
        final_state: ExecutionFinalState::NeedsInput,
        user_response: ask_for_missing_response(&fields, prompt.as_deref()),
    }
}

fn finish_rejected_route(reason: String) -> ExecutionOutcome {
    PlanExecutionTrace::new(PlanIntent::DirectResponse, PlanConfidence::High).finish_with_step(
        ExecutionStepKind::Rejected {
            reason: reason.clone(),
        },
        ExecutionFinalState::Rejected,
        reject_route_response(&reason),
    )
}

async fn execute_step(
    step: PlanStep,
    user_text: &str,
    last_output: Option<String>,
    ctx: &PlanExecutionContext<'_>,
) -> Result<StepExecutionDirective> {
    match step {
        PlanStep::CallTool { call } => Ok(execute_call_tool_step(call, ctx).await),
        PlanStep::Respond { style, guidance } => {
            execute_respond_step(style, guidance, user_text, last_output, ctx).await
        }
        PlanStep::AskForMissing { fields, prompt } => {
            Ok(execute_ask_for_missing_step(fields, prompt))
        }
    }
}

pub(crate) async fn execute_conversational_route(
    dispatch: ExecutableRoute,
    ctx: PlanExecutionContext<'_>,
) -> Result<ExecutionOutcome> {
    match dispatch {
        ExecutableRoute::Reject { reason } => Ok(finish_rejected_route(reason)),
        ExecutableRoute::PlanAndExecute { plan } => execute_plan(plan, ctx).await,
    }
}

async fn execute_plan(
    plan: ExecutionPlan,
    ctx: PlanExecutionContext<'_>,
) -> Result<ExecutionOutcome> {
    let ExecutionPlan {
        intent,
        confidence,
        steps,
    } = plan;
    let mut state = PlanExecutionState::new(intent, confidence);
    let user_text = ctx.workset.text();

    for step in steps {
        match execute_step(step, &user_text, state.last_output(), &ctx).await? {
            StepExecutionDirective::Continue { step, output } => {
                state.apply_continue(step, output);
            }
            StepExecutionDirective::Finish {
                step,
                final_state,
                user_response,
            } => {
                return Ok(state.finish_with_step(step, final_state, user_response));
            }
        }
    }

    Ok(state.finish_with_last_output())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DiscordConfig, GeminiConfig, RuntimeConfig};
    use crate::execution_contract::ToolCallSpec;
    use serde_json::json;
    use tempfile::tempdir;

    fn test_config() -> Config {
        Config {
            gemini: GeminiConfig {
                api_key: "fake".to_string(),
                model: "fake-model".to_string(),
            },
            discord: DiscordConfig {
                token: "fake".to_string(),
                guild_id: None,
                channel_mappings: None,
            },
            runtime: RuntimeConfig::default(),
        }
    }

    fn test_ctx<'a>(
        workset: &'a Workset,
        base_path: &'a Path,
        config: &'a Config,
    ) -> PlanExecutionContext<'a> {
        PlanExecutionContext {
            workset,
            base_path,
            config,
            channel_id: "0",
            system_prompt: "test system prompt",
        }
    }

    #[tokio::test]
    async fn reject_route_sets_rejected_final_state() {
        let dir = tempdir().unwrap();
        let config = test_config();
        let workset = Workset::new(vec!["ignored".to_string()]);

        let outcome = execute_conversational_route(
            ExecutableRoute::Reject {
                reason: "requires unsupported live data".to_string(),
            },
            test_ctx(&workset, dir.path(), &config),
        )
        .await
        .unwrap();

        assert_eq!(outcome.final_state, ExecutionFinalState::Rejected);
        assert!(
            outcome
                .user_response
                .contains("requires unsupported live data")
        );
        assert_eq!(outcome.trace.steps.len(), 1);
        assert!(matches!(
            &outcome.trace.steps[0].step,
            ExecutionStepKind::Rejected { reason } if reason == "requires unsupported live data"
        ));
    }

    #[tokio::test]
    async fn ask_for_missing_sets_needs_input_final_state() {
        let dir = tempdir().unwrap();
        let config = test_config();
        let workset = Workset::new(vec!["book a flight".to_string()]);
        let plan = ExecutionPlan {
            intent: PlanIntent::MissingInputCollection,
            confidence: PlanConfidence::High,
            steps: vec![PlanStep::AskForMissing {
                fields: vec!["destination".to_string(), "date".to_string()],
                prompt: None,
            }],
        };

        let outcome = execute_conversational_route(
            ExecutableRoute::PlanAndExecute { plan },
            test_ctx(&workset, dir.path(), &config),
        )
        .await
        .unwrap();

        assert_eq!(outcome.final_state, ExecutionFinalState::NeedsInput);
        assert!(outcome.user_response.contains("`destination`"));
        assert!(outcome.user_response.contains("`date`"));
        assert_eq!(outcome.trace.steps.len(), 1);
        assert!(matches!(
            &outcome.trace.steps[0].step,
            ExecutionStepKind::RequestedMissingInput { fields, prompt_only }
                if !prompt_only
                    && fields == &vec!["destination".to_string(), "date".to_string()]
        ));
    }

    #[tokio::test]
    async fn tool_error_sets_failed_final_state() {
        let dir = tempdir().unwrap();
        let config = test_config();
        let workset = Workset::new(vec!["run something".to_string()]);
        let plan = ExecutionPlan {
            intent: PlanIntent::ToolExecution,
            confidence: PlanConfidence::High,
            steps: vec![PlanStep::CallTool {
                call: ToolCallSpec {
                    tool_name: "missing_tool".to_string(),
                    args: json!({}),
                },
            }],
        };

        let outcome = execute_conversational_route(
            ExecutableRoute::PlanAndExecute { plan },
            test_ctx(&workset, dir.path(), &config),
        )
        .await
        .unwrap();

        assert_eq!(outcome.final_state, ExecutionFinalState::Failed);
        assert!(
            outcome
                .user_response
                .contains("Task execution stopped because `missing_tool` failed")
        );
        assert_eq!(outcome.trace.steps.len(), 1);
        assert!(matches!(
            &outcome.trace.steps[0].step,
            ExecutionStepKind::CalledTool {
                tool_name,
                succeeded: false
            } if tool_name == "missing_tool"
        ));
    }

    #[tokio::test]
    async fn successful_tool_only_plan_sets_completed_final_state() {
        let dir = tempdir().unwrap();
        let config = test_config();
        let workset = Workset::new(vec!["list files".to_string()]);
        let plan = ExecutionPlan {
            intent: PlanIntent::ToolExecution,
            confidence: PlanConfidence::High,
            steps: vec![PlanStep::CallTool {
                call: ToolCallSpec {
                    tool_name: "ls".to_string(),
                    args: json!({ "path": "." }),
                },
            }],
        };

        let outcome = execute_conversational_route(
            ExecutableRoute::PlanAndExecute { plan },
            test_ctx(&workset, dir.path(), &config),
        )
        .await
        .unwrap();

        assert_eq!(outcome.final_state, ExecutionFinalState::Completed);
        assert_eq!(outcome.trace.steps.len(), 1);
        assert!(matches!(
            &outcome.trace.steps[0].step,
            ExecutionStepKind::CalledTool {
                succeeded: true,
                ..
            }
        ));
        assert!(
            outcome.user_response.contains("Directory . is empty.")
                || outcome.user_response.contains("FILE")
                || outcome.user_response.contains("DIR")
        );
        assert!(!outcome.user_response.contains("Result confidence is"));
    }

    #[tokio::test]
    async fn medium_confidence_completed_result_warns_user() {
        let dir = tempdir().unwrap();
        let config = test_config();
        let workset = Workset::new(vec!["list files".to_string()]);
        let plan = ExecutionPlan {
            intent: PlanIntent::ToolExecution,
            confidence: PlanConfidence::Medium,
            steps: vec![PlanStep::CallTool {
                call: ToolCallSpec {
                    tool_name: "ls".to_string(),
                    args: json!({ "path": "." }),
                },
            }],
        };

        let outcome = execute_conversational_route(
            ExecutableRoute::PlanAndExecute { plan },
            test_ctx(&workset, dir.path(), &config),
        )
        .await
        .unwrap();

        assert_eq!(outcome.final_state, ExecutionFinalState::Completed);
        assert!(
            outcome
                .user_response
                .contains("Note: Result confidence is medium.")
        );
    }
}
