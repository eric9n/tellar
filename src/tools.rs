/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/tools.rs
 * Responsibility: Core tool definitions, dispatch, and tool safety constraints.
 */

use crate::config::Config;
use crate::llm;
use crate::skills::{self, SkillMetadata};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub(crate) struct ToolExecutionResult {
    pub output: String,
    pub is_error: bool,
}

impl ToolExecutionResult {
    pub(crate) fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    pub(crate) fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
        }
    }
}

pub(crate) const CORE_TOOL_NAMES: &[&str] = &["ls", "find", "grep", "read", "write", "edit", "exec"];

#[derive(Default)]
pub(crate) struct ToolBatchState {
    pub(crate) last_call_signature: Option<String>,
    pub(crate) last_observation_signature: Option<String>,
    pub(crate) no_new_info_streak: usize,
    pub(crate) repeated_error_streak: usize,
}

pub(crate) fn is_read_only_tool(name: &str) -> bool {
    matches!(name, "ls" | "find" | "grep" | "read")
}

pub(crate) fn is_write_tool(name: &str) -> bool {
    matches!(name, "write" | "edit")
}

pub(crate) fn tool_call_signature(call: &llm::ToolCallRequest) -> String {
    format!(
        "{}:{}",
        call.name,
        serde_json::to_string(&call.args).unwrap_or_else(|_| "{}".to_string())
    )
}

pub(crate) fn tool_observation_signature(result: &ToolExecutionResult) -> String {
    format!("{}:{}", result.is_error, result.output)
}

pub(crate) fn push_tool_result_message(
    messages: &mut Vec<llm::Message>,
    call: &llm::ToolCallRequest,
    observation: &ToolExecutionResult,
) {
    let response_key = if observation.is_error { "error" } else { "output" };
    messages.push(llm::Message {
        role: llm::MessageRole::ToolResult,
        parts: vec![llm::MultimodalPart::function_response(
            &call.name,
            json!({ response_key: observation.output }),
            Some(call.id.clone()),
        )],
    });
}

pub(crate) fn push_system_note(messages: &mut Vec<llm::Message>, note: impl Into<String>) {
    messages.push(llm::Message {
        role: llm::MessageRole::User,
        parts: vec![llm::MultimodalPart::text(format!("[System Note] {}", note.into()))],
    });
}

pub(crate) fn skip_remaining_tool_calls(
    messages: &mut Vec<llm::Message>,
    calls: &[llm::ToolCallRequest],
    start_index: usize,
    reason: &str,
) {
    for call in calls.iter().skip(start_index) {
        push_tool_result_message(messages, call, &ToolExecutionResult::error(reason.to_string()));
    }
}

fn normalize_path(path: &str) -> &str {
    let p = path.strip_prefix("guild/").unwrap_or(path);
    p.strip_prefix("./").unwrap_or(p)
}

pub(crate) fn is_path_safe(base: &Path, rel: &str) -> bool {
    if rel.contains("..") || rel.starts_with("/") {
        return false;
    }

    let base_real = match fs::canonicalize(base) {
        Ok(path) => path,
        Err(_) => return false,
    };

    let target = base.join(rel);
    if target.exists() {
        return fs::canonicalize(target)
            .map(|path| path.starts_with(&base_real))
            .unwrap_or(false);
    }

    let parent = match target.parent() {
        Some(path) => path,
        None => return false,
    };

    fs::canonicalize(parent)
        .map(|path| path.starts_with(&base_real))
        .unwrap_or(false)
}

fn require_path_arg<'a>(args: &'a Value, field: &str) -> Result<&'a str, ToolExecutionResult> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| ToolExecutionResult::error(format!("Error: Missing required argument `{}`.", field)))
}

fn optional_path_arg<'a>(args: &'a Value, field: &str, default: &'a str) -> &'a str {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .unwrap_or(default)
}

fn collect_paths(
    base_path: &Path,
    current_path: &Path,
    rel_display: &str,
    recursive: bool,
    max_depth: usize,
    current_depth: usize,
    out: &mut Vec<(String, PathBuf)>,
) -> std::io::Result<()> {
    if current_path.is_file() {
        out.push((rel_display.to_string(), current_path.to_path_buf()));
        return Ok(());
    }

    let mut entries: Vec<_> = fs::read_dir(current_path)?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let entry_path = entry.path();
        let display = entry_path
            .strip_prefix(base_path)
            .ok()
            .and_then(|path| path.to_str())
            .unwrap_or(rel_display)
            .replace('\\', "/");

        out.push((display.clone(), entry_path.clone()));

        if recursive && entry_path.is_dir() && current_depth < max_depth {
            collect_paths(
                base_path,
                &entry_path,
                &display,
                recursive,
                max_depth,
                current_depth + 1,
                out,
            )?;
        }
    }

    Ok(())
}

pub(crate) fn run_ls_tool(args: &Value, base_path: &Path) -> ToolExecutionResult {
    let rel_path = normalize_path(optional_path_arg(args, "path", "."));
    if !is_path_safe(base_path, rel_path) {
        return ToolExecutionResult::error("Error: Access denied. Path must be within the guild directory.");
    }

    let recursive = args.get("recursive").and_then(Value::as_bool).unwrap_or(false);
    let max_depth = args.get("maxDepth").and_then(Value::as_u64).unwrap_or(2) as usize;
    let target = if rel_path == "." { base_path.to_path_buf() } else { base_path.join(rel_path) };

    if !target.exists() {
        return ToolExecutionResult::error(format!("Error: Path not found: {}", rel_path));
    }

    if target.is_file() {
        return match fs::metadata(&target) {
            Ok(meta) => ToolExecutionResult::success(format!("FILE {} ({} bytes)", rel_path, meta.len())),
            Err(e) => ToolExecutionResult::error(format!("Error reading metadata: {}", e)),
        };
    }

    let mut paths = Vec::new();
    if let Err(e) = collect_paths(base_path, &target, rel_path, recursive, max_depth, 0, &mut paths) {
        return ToolExecutionResult::error(format!("Error listing path: {}", e));
    }

    if paths.is_empty() {
        ToolExecutionResult::success(format!("Directory {} is empty.", rel_path))
    } else {
        let lines = paths
            .into_iter()
            .map(|(display, path)| {
                let kind = if path.is_dir() { "DIR" } else { "FILE" };
                format!("{} {}", kind, display)
            })
            .collect::<Vec<_>>()
            .join("\n");
        ToolExecutionResult::success(lines)
    }
}

pub(crate) fn run_find_tool(args: &Value, base_path: &Path) -> ToolExecutionResult {
    let name = match args.get("name").and_then(Value::as_str).filter(|v| !v.is_empty()) {
        Some(value) => value,
        None => return ToolExecutionResult::error("Error: Missing required argument `name`."),
    };
    let rel_path = normalize_path(optional_path_arg(args, "path", "."));
    if !is_path_safe(base_path, rel_path) {
        return ToolExecutionResult::error("Error: Access denied. Path must be within the guild directory.");
    }

    let recursive = args.get("recursive").and_then(Value::as_bool).unwrap_or(true);
    let case_sensitive = args.get("caseSensitive").and_then(Value::as_bool).unwrap_or(false);
    let max_matches = args.get("maxMatches").and_then(Value::as_u64).unwrap_or(50) as usize;
    let max_depth = args.get("maxDepth").and_then(Value::as_u64).unwrap_or(8) as usize;
    let target = if rel_path == "." { base_path.to_path_buf() } else { base_path.join(rel_path) };

    if !target.exists() {
        return ToolExecutionResult::error(format!("Error: Path not found: {}", rel_path));
    }

    let mut paths = Vec::new();
    if let Err(e) = collect_paths(base_path, &target, rel_path, recursive, max_depth, 0, &mut paths) {
        return ToolExecutionResult::error(format!("Error scanning path: {}", e));
    }

    let needle = if case_sensitive {
        name.to_string()
    } else {
        name.to_lowercase()
    };

    let mut matches = Vec::new();
    for (display, path) in paths {
        let Some(file_name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        let haystack = if case_sensitive {
            file_name.to_string()
        } else {
            file_name.to_lowercase()
        };
        if haystack.contains(&needle) {
            let kind = if path.is_dir() { "DIR" } else { "FILE" };
            matches.push(format!("{} {}", kind, display));
            if matches.len() >= max_matches {
                break;
            }
        }
    }

    if matches.is_empty() {
        ToolExecutionResult::success(format!("No paths matching `{}` under {}.", name, rel_path))
    } else {
        ToolExecutionResult::success(matches.join("\n"))
    }
}

pub(crate) fn run_grep_tool(args: &Value, base_path: &Path) -> ToolExecutionResult {
    let pattern = match args.get("pattern").and_then(Value::as_str).filter(|v| !v.is_empty()) {
        Some(value) => value,
        None => return ToolExecutionResult::error("Error: Missing required argument `pattern`."),
    };
    let rel_path = normalize_path(optional_path_arg(args, "path", "."));
    if !is_path_safe(base_path, rel_path) {
        return ToolExecutionResult::error("Error: Access denied. Path must be within the guild directory.");
    }

    let recursive = args.get("recursive").and_then(Value::as_bool).unwrap_or(true);
    let case_sensitive = args.get("caseSensitive").and_then(Value::as_bool).unwrap_or(false);
    let max_matches = args.get("maxMatches").and_then(Value::as_u64).unwrap_or(50) as usize;
    let target = if rel_path == "." { base_path.to_path_buf() } else { base_path.join(rel_path) };

    if !target.exists() {
        return ToolExecutionResult::error(format!("Error: Path not found: {}", rel_path));
    }

    let mut paths = Vec::new();
    if let Err(e) = collect_paths(base_path, &target, rel_path, recursive, usize::MAX, 0, &mut paths) {
        return ToolExecutionResult::error(format!("Error scanning path: {}", e));
    }

    let needle = if case_sensitive {
        pattern.to_string()
    } else {
        pattern.to_lowercase()
    };

    let mut matches = Vec::new();
    for (display, path) in paths {
        if !path.is_file() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };

        for (index, line) in content.lines().enumerate() {
            let haystack = if case_sensitive {
                line.to_string()
            } else {
                line.to_lowercase()
            };
            if haystack.contains(&needle) {
                matches.push(format!("{}:{}: {}", display, index + 1, line));
                if matches.len() >= max_matches {
                    return ToolExecutionResult::success(matches.join("\n"));
                }
            }
        }
    }

    if matches.is_empty() {
        ToolExecutionResult::success(format!("No matches for `{}` under {}.", pattern, rel_path))
    } else {
        ToolExecutionResult::success(matches.join("\n"))
    }
}

fn core_tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "ls",
            "description": "List files and directories inside the guild. Use this for discovery instead of shell access.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to inspect, relative to guild root. Defaults to '.'" },
                    "recursive": { "type": "boolean", "description": "Whether to descend into subdirectories" },
                    "maxDepth": { "type": "number", "description": "Maximum recursion depth when recursive=true. Defaults to 2" }
                }
            }
        }),
        json!({
            "name": "find",
            "description": "Find files or directories by name. Use this when you do not know the exact path yet.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Substring to match in file or directory names" },
                    "path": { "type": "string", "description": "Path to search under, relative to guild root. Defaults to '.'" },
                    "recursive": { "type": "boolean", "description": "Whether to search subdirectories. Defaults to true" },
                    "caseSensitive": { "type": "boolean", "description": "Whether matching should be case sensitive" },
                    "maxMatches": { "type": "number", "description": "Maximum number of results to return. Defaults to 50" },
                    "maxDepth": { "type": "number", "description": "Maximum recursion depth when recursive=true. Defaults to 8" }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "grep",
            "description": "Search text files for a string pattern. Use this to find filenames, symbols, IDs, or text snippets inside the guild.",
            "parameters": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "The text to search for" },
                    "path": { "type": "string", "description": "Path to search under, relative to guild root. Defaults to '.'" },
                    "recursive": { "type": "boolean", "description": "Whether to search subdirectories. Defaults to true" },
                    "caseSensitive": { "type": "boolean", "description": "Whether matching should be case sensitive" },
                    "maxMatches": { "type": "number", "description": "Maximum number of matches to return. Defaults to 50" }
                },
                "required": ["pattern"]
            }
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
            "name": "exec",
            "description": "Run a host shell command. This is a privileged tool: when runtime.privileged=false it rejects immediately. Use this for absolute host paths, system scripts, or cross-workspace operations.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute on the host" }
                },
                "required": ["command"]
            }
        }),
    ]
}

async fn run_exec_tool(args: &Value, base_path: &Path, config: &Config) -> ToolExecutionResult {
    let command = match args.get("command").and_then(Value::as_str).filter(|v| !v.is_empty()) {
        Some(value) => value,
        None => return ToolExecutionResult::error("Error: Missing required argument `command`."),
    };

    if !config.runtime.privileged {
        return ToolExecutionResult::error(
            "Error: `exec` is disabled because runtime.privileged=false. Explain the limitation or enable privileged mode.",
        );
    }

    let output = match config.runtime.exec_mode {
        crate::config::ExecMode::Unrestricted => {
            Command::new("sh")
                .arg("-lc")
                .arg(command)
                .current_dir(base_path)
                .env("TELLAR_WORKSPACE", base_path)
                .output()
                .await
        }
    };

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut combined = String::new();
            if !stdout.trim().is_empty() {
                combined.push_str(stdout.trim_end());
            }
            if !stderr.trim().is_empty() {
                if !combined.is_empty() {
                    combined.push_str("\n");
                }
                combined.push_str("[stderr]\n");
                combined.push_str(stderr.trim_end());
            }
            if combined.is_empty() {
                combined = "(no output)".to_string();
            }

            if output.status.success() {
                ToolExecutionResult::success(combined)
            } else {
                let code = output
                    .status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "terminated by signal".to_string());
                ToolExecutionResult::error(format!("Command failed ({}):\n{}", code, combined))
            }
        }
        Err(e) => ToolExecutionResult::error(format!("Error executing command: {}", e)),
    }
}

pub fn mask_sensitive_data(text: &str, config: &Config) -> String {
    let mut masked = text.to_string();

    if !config.gemini.api_key.is_empty() && config.gemini.api_key.len() > 10 {
        masked = masked.replace(&config.gemini.api_key, "[REDACTED_GEMINI_KEY]");
    }

    if !config.discord.token.is_empty() && config.discord.token.len() > 10 {
        masked = masked.replace(&config.discord.token, "[REDACTED_DISCORD_TOKEN]");
    }

    masked
}

pub(crate) async fn dispatch_tool(
    name: &str,
    args: &Value,
    base_path: &Path,
    config: &Config,
) -> ToolExecutionResult {
    let output = match name {
        "ls" => run_ls_tool(args, base_path),
        "find" => run_find_tool(args, base_path),
        "grep" => run_grep_tool(args, base_path),
        "read" => {
            let rel_path = match require_path_arg(args, "path") {
                Ok(path) => normalize_path(path),
                Err(err) => return err,
            };
            if !is_path_safe(base_path, rel_path) {
                return ToolExecutionResult::error("Error: Access denied. Path must be within the guild directory.");
            }

            let offset = args["offset"].as_u64().unwrap_or(1) as usize;
            let limit = args["limit"].as_u64().unwrap_or(800) as usize;
            if offset == 0 {
                return ToolExecutionResult::error("Error: `offset` must be >= 1.");
            }

            let file_path = base_path.join(rel_path);
            if !file_path.exists() {
                ToolExecutionResult::error(format!("Error: File not found: {}", rel_path))
            } else {
                match fs::read_to_string(&file_path) {
                    Ok(content) => {
                        let lines: Vec<&str> = content.lines().collect();
                        if offset > lines.len() {
                            ToolExecutionResult::error(format!(
                                "Error: offset {} is beyond file length {}",
                                offset,
                                lines.len()
                            ))
                        } else {
                            let end = std::cmp::min(offset - 1 + limit, lines.len());
                            ToolExecutionResult::success(lines[(offset - 1)..end].join("\n"))
                        }
                    }
                    Err(e) => ToolExecutionResult::error(format!("Error reading file: {}", e)),
                }
            }
        }
        "write" => {
            let rel_path = match require_path_arg(args, "path") {
                Ok(path) => normalize_path(path),
                Err(err) => return err,
            };
            if !is_path_safe(base_path, rel_path) {
                return ToolExecutionResult::error("Error: Access denied. Path must be within the guild directory.");
            }

            let content = match args.get("content").and_then(Value::as_str) {
                Some(content) => content,
                None => return ToolExecutionResult::error("Error: Missing required argument `content`."),
            };
            let full_path = base_path.join(rel_path);

            if let Some(parent) = full_path.parent() {
                let _ = fs::create_dir_all(parent);
            }

            match fs::write(&full_path, content) {
                Ok(_) => ToolExecutionResult::success(format!("Successfully wrote to {}", rel_path)),
                Err(e) => ToolExecutionResult::error(format!("Error writing file: {}", e)),
            }
        }
        "edit" => {
            let rel_path = match require_path_arg(args, "path") {
                Ok(path) => normalize_path(path),
                Err(err) => return err,
            };
            if !is_path_safe(base_path, rel_path) {
                return ToolExecutionResult::error("Error: Access denied. Path must be within the guild directory.");
            }

            let old_text = match args.get("oldText").and_then(Value::as_str) {
                Some(content) if !content.is_empty() => content,
                _ => return ToolExecutionResult::error("Error: Missing required argument `oldText`."),
            };
            let new_text = match args.get("newText").and_then(Value::as_str) {
                Some(content) => content,
                None => return ToolExecutionResult::error("Error: Missing required argument `newText`."),
            };

            match fs::read_to_string(base_path.join(rel_path)) {
                Ok(content) => {
                    let occurrences: Vec<_> = content.matches(old_text).collect();
                    if occurrences.len() == 1 {
                        let new_content = content.replace(old_text, new_text);
                        match fs::write(base_path.join(rel_path), new_content) {
                            Ok(_) => ToolExecutionResult::success(format!("Successfully edited {}", rel_path)),
                            Err(e) => ToolExecutionResult::error(format!("Error writing file: {}", e)),
                        }
                    } else if occurrences.is_empty() {
                        ToolExecutionResult::error(format!("Error: oldText not found in {}", rel_path))
                    } else {
                        ToolExecutionResult::error(format!(
                            "Error: oldText is not unique in {} (found {} occurrences)",
                            rel_path,
                            occurrences.len()
                        ))
                    }
                }
                Err(_e) => ToolExecutionResult::error(format!("Error: File not found: {}", rel_path)),
            }
        }
        "exec" => run_exec_tool(args, base_path, config).await,
        _ => {
            let skills = SkillMetadata::discover_skills(base_path);
            let mut skill_out = ToolExecutionResult::error(format!("Error: Unknown tool `{}`", name));
            for (meta, dir) in skills {
                if let Some(tool) = meta.tools.get(name) {
                    skill_out = match skills::execute_skill_tool(tool, &dir, base_path, args, config).await {
                        Ok(out) => ToolExecutionResult::success(out),
                        Err(e) => ToolExecutionResult::error(format!("Error executing skill tool `{}`: {}", name, e)),
                    };
                    break;
                }
            }
            skill_out
        }
    };

    ToolExecutionResult {
        output: truncate_output(output.output, config.runtime.max_tool_output_bytes),
        is_error: output.is_error,
    }
}

fn truncate_output(output: String, limit: usize) -> String {
    if limit == 0 {
        return output;
    }

    if output.len() > limit {
        let mut prefix_end = limit / 2;
        while prefix_end > 0 && !output.is_char_boundary(prefix_end) {
            prefix_end -= 1;
        }

        let mut suffix_start = output.len().saturating_sub(limit / 2);
        while suffix_start < output.len() && !output.is_char_boundary(suffix_start) {
            suffix_start += 1;
        }

        let prefix = &output[..prefix_end];
        let suffix = &output[suffix_start..];

        format!(
            "{} ... [TRUNCATED {} bytes] ... {}\n\n💡 **Hint**: Data is too large for the session history. Narrow the path, reduce the line window, or search for a more specific pattern before reading again.",
            prefix,
            output.len() - (prefix_end + (output.len() - suffix_start)),
            suffix
        )
    } else {
        output
    }
}

pub(crate) fn get_tool_definitions(base_path: &Path, _config: &Config) -> Value {
    let mut tools = json!(core_tool_definitions());

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
    async fn test_exec_tool_rejects_when_privileged_mode_is_disabled() {
        let dir = tempdir().unwrap();
        let result = dispatch_tool("exec", &json!({ "command": "pwd" }), dir.path(), &test_config()).await;

        assert!(result.is_error);
        assert!(result.output.contains("runtime.privileged=false"));
    }

    #[tokio::test]
    async fn test_exec_tool_runs_when_privileged_mode_is_enabled() {
        let dir = tempdir().unwrap();
        let mut config = test_config();
        config.runtime.privileged = true;
        let result =
            dispatch_tool("exec", &json!({ "command": "printf host-ok" }), dir.path(), &config).await;

        assert!(!result.is_error);
        assert_eq!(result.output, "host-ok");
    }

    #[tokio::test]
    async fn test_dispatch_tool_rejects_missing_write_content() {
        let dir = tempdir().unwrap();
        let result = dispatch_tool("write", &json!({ "path": "notes.txt" }), dir.path(), &test_config()).await;

        assert!(result.is_error);
        assert!(result.output.contains("Missing required argument `content`"));
        assert!(!dir.path().join("notes.txt").exists());
    }

    #[test]
    fn test_find_ls_and_grep_tools_work_without_shell() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join("docs").join("alpha.txt"), "hello\nfind me\n").unwrap();
        fs::write(dir.path().join("docs").join("beta.txt"), "nothing\n").unwrap();

        let find_result = run_find_tool(&json!({ "name": "alpha", "path": "docs" }), dir.path());
        assert!(!find_result.is_error);
        assert!(find_result.output.contains("FILE docs/alpha.txt"));

        let ls_result = run_ls_tool(&json!({ "path": "docs", "recursive": true }), dir.path());
        assert!(!ls_result.is_error);
        assert!(ls_result.output.contains("FILE docs/alpha.txt"));

        let grep_result = run_grep_tool(&json!({ "pattern": "find me", "path": "docs" }), dir.path());
        assert!(!grep_result.is_error);
        assert!(grep_result.output.contains("docs/alpha.txt:2: find me"));
    }

    #[test]
    fn test_is_path_safe_rejects_symlink_escape() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let escape_target = outside.path().join("outside.txt");
        fs::write(&escape_target, "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&escape_target, dir.path().join("escape.txt")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&escape_target, dir.path().join("escape.txt")).unwrap();

        assert!(!is_path_safe(dir.path(), "escape.txt"));
    }
}
