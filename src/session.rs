/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/session.rs
 * Responsibility: Orchestrate task routing and finite plan execution for ritual and conversational work.
 */

use crate::config::Config;
use crate::execution_contract::{
    ConversationalLoopOutcome, ConversationalLoopState, ExecutionOutcome, RequestRoute,
};
use crate::input::{Workset, collect_pending_workset};
use crate::plan_executor::{PlanExecutionContext, execute_conversational_route};
use crate::prompt_context::load_unified_prompt;
use crate::router::plan_conversational_request;
use crate::task_policy::apply_request_route_policy;
use crate::task_response::no_new_workset_response;
use std::path::Path;
use std::sync::Arc;

async fn resolve_task_route(
    base_path: &Path,
    config: Arc<Config>,
    workset: &Workset,
    execution_label: &str,
    fallback_prompt: &str,
) -> RequestRoute {
    let policy_decision = apply_request_route_policy(
        match plan_conversational_request(base_path, config, workset).await {
            Ok(route) => route,
            Err(err) => {
                eprintln!(
                    "⚠️ {} router failed, returning clarification request: {}",
                    execution_label, err
                );
                RequestRoute::NeedsInput {
                    fields: Vec::new(),
                    prompt: Some(fallback_prompt.to_string()),
                }
            }
        },
    );

    if let Some(note) = policy_decision.log_note() {
        eprintln!("🧭 {} routing note: {}", execution_label, note);
    }

    policy_decision.route
}

async fn execute_task_route(
    workset: &Workset,
    base_path: &Path,
    config: Arc<Config>,
    channel_id: &str,
    system_prompt: &str,
    execution_label: &str,
    route: RequestRoute,
) -> anyhow::Result<ExecutionOutcome> {
    let outcome = execute_conversational_route(
        route.into_executable(),
        PlanExecutionContext {
            workset,
            base_path,
            config,
            channel_id,
            system_prompt,
        },
    )
    .await?;

    eprintln!(
        "🧭 {} plan executed: final_state={} success={} {}",
        execution_label,
        outcome.final_state.label(),
        outcome.is_terminal_success(),
        outcome.trace.summarize()
    );

    Ok(outcome)
}

pub(crate) async fn execute_ritual_step(
    task: &str,
    _full_context: &str,
    _path: &Path,
    base_path: &Path,
    config: Arc<Config>,
    channel_id: &str,
) -> anyhow::Result<ExecutionOutcome> {
    let system_prompt_str = load_unified_prompt(base_path, channel_id);
    let ritual_workset = Workset::new(vec![task.to_string()]);
    let route = resolve_task_route(
        base_path,
        Arc::clone(&config),
        &ritual_workset,
        "Ritual",
        "This ritual step is not ready to execute. Provide the exact target or missing inputs.",
    )
    .await;

    execute_task_route(
        &ritual_workset,
        base_path,
        config,
        channel_id,
        &system_prompt_str,
        "Ritual",
        route,
    )
    .await
}

pub(crate) async fn run_conversational_loop(
    full_context: &str,
    _path: &Path,
    base_path: &Path,
    config: Arc<Config>,
    trigger_id: Option<String>,
    channel_id: &str,
) -> anyhow::Result<ConversationalLoopOutcome> {
    let workset = collect_pending_workset(full_context, trigger_id.as_deref());
    if workset.is_empty() {
        return Ok(ConversationalLoopOutcome {
            user_response: no_new_workset_response(),
            state: ConversationalLoopState::NoNewWorkset,
            trace: None,
        });
    }

    let system_prompt_str = load_unified_prompt(base_path, channel_id);
    let route = resolve_task_route(
        base_path,
        Arc::clone(&config),
        &workset,
        "Conversational",
        "This task is not ready to execute. Provide the exact target or missing inputs.",
    )
    .await;
    let outcome = execute_task_route(
        &workset,
        base_path,
        config,
        channel_id,
        &system_prompt_str,
        "Conversational",
        route,
    )
    .await?;

    Ok(ConversationalLoopOutcome {
        user_response: outcome.user_response,
        state: ConversationalLoopState::Planned(outcome.final_state),
        trace: Some(outcome.trace.view()),
    })
}
