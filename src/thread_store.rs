/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/thread_store.rs
 * Responsibility: Thread file persistence helpers, log entry formatting, and archive path rules.
 */

use crate::execution_contract::ExecutionOutcome;
use once_cell::sync::Lazy;
use regex::Regex;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

static ANY_TODO_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"- \[ \]").expect("valid todo regex"));

pub(crate) fn append_task_result_log(
    content: &str,
    task_line: &str,
    outcome: &ExecutionOutcome,
    timestamp: &str,
) -> (String, bool) {
    if outcome.is_terminal_success() {
        let updated_line = task_line.replace("[ ]", "[x]");
        let log_entry = format!(
            "\n> [{}] Execution result: {}",
            timestamp, outcome.user_response
        );
        let mut next = content.replacen(task_line, &updated_line, 1);
        next.push_str(&log_entry);
        (next, true)
    } else {
        let log_entry = format!(
            "\n> [{}] ❌ Task failed ({}): {}",
            timestamp,
            outcome.final_state.label(),
            outcome.user_response
        );
        let mut next = content.to_string();
        next.push_str(&log_entry);
        (next, false)
    }
}

pub(crate) fn append_internal_task_error_log(
    content: &str,
    timestamp: &str,
    error: &str,
) -> String {
    let mut next = content.to_string();
    next.push_str(&format!(
        "\n> [{}] ❌ Task failed (InternalError): {}",
        timestamp, error
    ));
    next
}

pub(crate) fn append_discord_response_log(
    content: &str,
    bot_name: &str,
    bot_id: &str,
    timestamp: &str,
    msg_id: &str,
    user_response: &str,
) -> String {
    let mut next = content.to_string();
    next.push_str(&format!(
        "\n---\n**Author**: {} (ID: {}) | **Time**: {} | **Message ID**: {}\n\n{}\n",
        bot_name, bot_id, timestamp, msg_id, user_response
    ));
    next
}

pub(crate) fn append_local_response_log(
    content: &str,
    timestamp: &str,
    user_response: &str,
) -> String {
    let mut next = content.to_string();
    next.push_str(&format!(
        "\n\n> [Tellar] ({}): {}\n",
        timestamp, user_response
    ));
    next
}

pub(crate) fn append_processing_error_log(content: &str, timestamp: &str, error: &str) -> String {
    let mut next = content.to_string();
    next.push_str(&format!(
        "\n\n> [Tellar] ({}): ❌ Error processing request: {}",
        timestamp, error
    ));
    next
}

pub(crate) fn should_archive_thread(content: &str, schedule: Option<&str>) -> bool {
    let schedule_value = schedule.unwrap_or("").trim();
    if !schedule_value.is_empty() {
        return false;
    }

    !ANY_TODO_RE.is_match(content)
}

pub(crate) fn history_destination(parent: &Path, file_name: &OsStr, date: &str) -> PathBuf {
    parent.join("history").join(date).join(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_contract::{
        ExecutionFinalState, ExecutionTrace, PlanConfidence, PlanIntent,
    };

    #[test]
    fn test_append_task_result_log_marks_completed_task() {
        let content = "---\nstatus: open\n---\n- [ ] Ship release";
        let outcome = ExecutionOutcome {
            final_state: ExecutionFinalState::Completed,
            user_response: "Release shipped successfully".to_string(),
            trace: ExecutionTrace {
                intent: PlanIntent::ToolExecutionWithResponse,
                confidence: PlanConfidence::High,
                steps: vec![],
            },
        };
        let (updated, completed) = append_task_result_log(
            content,
            "- [ ] Ship release",
            &outcome,
            "2026-02-27 12:00:00",
        );

        assert!(completed);
        assert!(updated.contains("- [x] Ship release"));
        assert!(updated.contains("Execution result: Release shipped successfully"));
    }

    #[test]
    fn test_append_task_result_log_only_marks_first_matching_task() {
        let content = "---\nstatus: open\n---\n- [ ] Ship release\n- [ ] Ship release";
        let outcome = ExecutionOutcome {
            final_state: ExecutionFinalState::Completed,
            user_response: "Release shipped successfully".to_string(),
            trace: ExecutionTrace {
                intent: PlanIntent::ToolExecutionWithResponse,
                confidence: PlanConfidence::High,
                steps: vec![],
            },
        };

        let (updated, completed) = append_task_result_log(
            content,
            "- [ ] Ship release",
            &outcome,
            "2026-02-27 12:00:00",
        );

        assert!(completed);
        assert_eq!(updated.matches("- [x] Ship release").count(), 1);
        assert_eq!(updated.matches("- [ ] Ship release").count(), 1);
    }

    #[test]
    fn test_append_task_result_log_keeps_failed_task_open() {
        let content = "---\nstatus: open\n---\n- [ ] Ship release";
        let outcome = ExecutionOutcome {
            final_state: ExecutionFinalState::Failed,
            user_response: "network failed".to_string(),
            trace: ExecutionTrace {
                intent: PlanIntent::ToolExecution,
                confidence: PlanConfidence::High,
                steps: vec![],
            },
        };
        let (updated, completed) = append_task_result_log(
            content,
            "- [ ] Ship release",
            &outcome,
            "2026-02-27 12:00:00",
        );

        assert!(!completed);
        assert!(updated.contains("- [ ] Ship release"));
        assert!(updated.contains("❌ Task failed (Failed): network failed"));
    }

    #[test]
    fn test_should_archive_thread_requires_no_schedule_and_no_open_todos() {
        assert!(should_archive_thread(
            "---\nstatus: done\n---\n- [x] Finished",
            None
        ));
        assert!(!should_archive_thread(
            "---\nstatus: done\n---\n- [ ] Pending",
            None
        ));
        assert!(!should_archive_thread(
            "---\nstatus: done\n---\n- [x] Finished",
            Some("0 * * * *")
        ));
    }

    #[test]
    fn test_history_destination_builds_expected_path() {
        let parent = Path::new("/tmp/channel");
        let dest = history_destination(parent, OsStr::new("thread.md"), "2026-02-27");
        assert_eq!(dest, Path::new("/tmp/channel/history/2026-02-27/thread.md"));
    }
}
