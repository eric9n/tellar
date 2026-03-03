/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/session.rs
 * Responsibility: Assemble role sessions from prompts, memory, and multimodal context.
 */

use crate::agent_loop::run_agent_loop;
use crate::config::Config;
use crate::conversation_context::{
    build_conversational_agent_context, build_react_prompt_context,
};
use crate::context::load_unified_prompt;
use crate::input::collect_pending_workset;
use crate::plan_executor::execute_conversational_route;
use crate::router::{plan_conversational_request, RequestRoute};
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
    let initial_messages =
        build_react_prompt_context(&mut system_prompt_str, task, full_context, path, base_path, config);

    run_agent_loop(
        initial_messages,
        path,
        base_path,
        config,
        channel_id,
        &system_prompt_str,
        true,
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

    let workset = collect_pending_workset(full_context, trigger_id.as_deref());
    if workset.is_empty() {
        return Ok(
            "I did not find any new user request content to process beyond the wake signal."
                .to_string(),
        );
    }
    let workset_text = workset.text();
    let workset_trimmed = workset_text.trim();

    let routed = match plan_conversational_request(base_path, config, &workset).await {
        Ok(route) => route,
        Err(err) => {
            eprintln!("⚠️ Conversational router failed, falling back to agent loop: {}", err);
            RequestRoute::Agent
        }
    };

    if !matches!(routed, RequestRoute::Agent) {
        return execute_conversational_route(
            routed,
            base_path,
            config,
            channel_id,
            &system_prompt_str,
            workset_trimmed,
        )
        .await;
    }
    let initial_messages = build_conversational_agent_context(
        &mut system_prompt_str,
        workset_trimmed,
        full_context,
        path,
        base_path,
        config,
    );

    run_agent_loop(
        initial_messages,
        path,
        base_path,
        config,
        channel_id,
        &system_prompt_str,
        false,
    )
    .await
}
