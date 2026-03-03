/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/conversation_policy.rs
 * Responsibility: Conversation-specific execution boundaries and prompt guidance.
 */

use crate::config::Config;
use regex::Regex;

pub(crate) fn execution_boundary_note(text: &str, config: &Config) -> Option<String> {
    let has_unix_abs_path = Regex::new(r#"(^|[\s`'"(])/(?:[^/\s]+/)*[^/\s]+"#)
        .expect("valid absolute path regex")
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

pub(crate) fn react_objective_instruction(explicit_skill_match: bool) -> &'static str {
    if explicit_skill_match {
        "Use native tool calling. The user explicitly referenced a skill or skill tool. \
Prioritize the matching discovered skill/tool before generic file exploration. If the named \
skill/tool returns a useful result, answer the user directly instead of continuing with \
find/ls/grep/read. Only fall back to local cognition tools if the named skill cannot satisfy the request."
    } else {
        "Use native tool calling. Prefer `find` when the path is unknown, `ls` when the directory is known, then `grep` to narrow matches, then `read` before `write` or `edit`. Use a discovered skill only when the task needs domain-specific or external capabilities. Use `finish` when the step is complete."
    }
}

pub(crate) fn conversational_agent_instruction(explicit_skill_match: bool) -> &'static str {
    if explicit_skill_match {
        "Respond naturally. Use Markdown. The user explicitly named a skill or tool, so prioritize that matching discovered skill/tool first. If it returns a usable result, answer immediately instead of exploring with find/ls/grep/read. Only use local cognition tools when the named skill is insufficient or fails and you need to explain why."
    } else {
        "Respond naturally. Use Markdown. Prefer local cognition tools (`find`, `ls`, `grep`, `read`) before modifying files or invoking skills. Concise yet premium."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_config() -> Config {
        Config {
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
        }
    }

    #[test]
    fn test_execution_boundary_note_detects_absolute_path() {
        let note = execution_boundary_note("请读取 /root/process_intel.py", &fake_config()).unwrap();
        assert!(note.contains("Call `exec` first"));
    }

    #[test]
    fn test_execution_boundary_note_detects_attachment_request() {
        let note = execution_boundary_note("以附件发给我", &fake_config()).unwrap();
        assert!(note.contains("send_attachment"));
        assert!(note.contains("Do not paste the full file contents"));
    }

    #[test]
    fn test_execution_boundary_note_none_for_normal_request() {
        assert!(execution_boundary_note("Read channels/general/KNOWLEDGE.md", &fake_config()).is_none());
    }
}
