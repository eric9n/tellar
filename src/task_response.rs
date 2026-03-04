/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/task_response.rs
 * Responsibility: User-facing response phrasing for task routing and execution flows.
 */

use crate::execution_contract::{ExecutionFinalState, PlanConfidence};

pub(crate) fn no_new_workset_response() -> String {
    "I did not find a new task to process beyond the wake signal.".to_string()
}

pub(crate) fn reject_route_response(reason: &str) -> String {
    format!(
        "This task cannot be completed with the currently available capabilities. {}",
        reason
    )
}

pub(crate) fn tool_failure_response(tool_name: &str, output: &str) -> String {
    format!(
        "Task execution stopped because `{}` failed.\nReason:\n{}",
        tool_name, output
    )
}

pub(crate) fn respond_step_fallback(last_output: Option<String>) -> String {
    last_output.unwrap_or_else(|| {
        "Task execution produced a usable result, but no approved final response step was available in the plan.".to_string()
    })
}

pub(crate) fn ask_for_missing_response(fields: &[String], prompt: Option<&str>) -> String {
    if let Some(prompt) = prompt.map(str::trim).filter(|v| !v.is_empty()) {
        return prompt.to_string();
    }

    if fields.is_empty() {
        return "This task needs more detail before execution can continue.".to_string();
    }

    if fields.len() == 1 {
        return format!("I need `{}` before I can continue this task.", fields[0]);
    }

    let joined = fields
        .iter()
        .map(|f| format!("`{}`", f))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "I need these inputs before I can continue this task: {}.",
        joined
    )
}

pub(crate) fn append_result_confidence_notice(
    user_response: String,
    final_state: ExecutionFinalState,
    confidence: PlanConfidence,
) -> String {
    if final_state != ExecutionFinalState::Completed {
        return user_response;
    }

    let notice = match confidence {
        PlanConfidence::High => return user_response,
        PlanConfidence::Medium => {
            "Result confidence is medium. Verify important details before acting."
        }
        PlanConfidence::Low => {
            "Result confidence is low. Treat this as tentative and verify it before acting."
        }
    };

    format!("{}\n\nNote: {}", user_response, notice)
}
