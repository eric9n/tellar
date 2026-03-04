/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/tools.rs
 * Responsibility: Core tool definitions, dispatch, and tool safety constraints.
 */

use crate::config::Config;
use crate::delivery;
use crate::skills::{self, SkillMetadata};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone)]
struct ResolvedTargetPath {
    rel_path: String,
    target: PathBuf,
}

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

    pub(crate) fn with_truncated_output(mut self, limit: usize) -> Self {
        self.output = truncate_output(self.output, limit);
        self
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
        .ok_or_else(|| {
            ToolExecutionResult::error(format!("Error: Missing required argument `{}`.", field))
        })
}

fn require_string_arg<'a>(args: &'a Value, field: &str) -> Result<&'a str, ToolExecutionResult> {
    args.get(field).and_then(Value::as_str).ok_or_else(|| {
        ToolExecutionResult::error(format!("Error: Missing required argument `{}`.", field))
    })
}

fn require_non_empty_string_arg<'a>(
    args: &'a Value,
    field: &str,
) -> Result<&'a str, ToolExecutionResult> {
    require_string_arg(args, field).and_then(|value| {
        if value.is_empty() {
            Err(ToolExecutionResult::error(format!(
                "Error: Missing required argument `{}`.",
                field
            )))
        } else {
            Ok(value)
        }
    })
}

fn require_safe_rel_path<'a>(
    args: &'a Value,
    field: &str,
    base_path: &Path,
) -> Result<&'a str, ToolExecutionResult> {
    let rel_path = normalize_path(require_path_arg(args, field)?);
    if !is_path_safe(base_path, rel_path) {
        return Err(ToolExecutionResult::error(
            "Error: Access denied. Path must be within the guild directory.",
        ));
    }
    Ok(rel_path)
}

fn optional_path_arg<'a>(args: &'a Value, field: &str, default: &'a str) -> &'a str {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .unwrap_or(default)
}

fn resolve_optional_target_path(
    args: &Value,
    field: &str,
    default: &str,
    base_path: &Path,
) -> Result<ResolvedTargetPath, ToolExecutionResult> {
    let rel_path = normalize_path(optional_path_arg(args, field, default)).to_string();
    if !is_path_safe(base_path, &rel_path) {
        return Err(ToolExecutionResult::error(
            "Error: Access denied. Path must be within the guild directory.",
        ));
    }

    let target = if rel_path == "." {
        base_path.to_path_buf()
    } else {
        base_path.join(&rel_path)
    };

    if !target.exists() {
        return Err(ToolExecutionResult::error(format!(
            "Error: Path not found: {}",
            rel_path
        )));
    }

    Ok(ResolvedTargetPath { rel_path, target })
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

fn collect_target_paths(
    base_path: &Path,
    target: &ResolvedTargetPath,
    recursive: bool,
    max_depth: usize,
) -> Result<Vec<(String, PathBuf)>, ToolExecutionResult> {
    let mut paths = Vec::new();
    collect_paths(
        base_path,
        &target.target,
        &target.rel_path,
        recursive,
        max_depth,
        0,
        &mut paths,
    )
    .map_err(|e| ToolExecutionResult::error(format!("Error scanning path: {}", e)))?;
    Ok(paths)
}

pub(crate) fn run_ls_tool(args: &Value, base_path: &Path) -> ToolExecutionResult {
    let target = match resolve_optional_target_path(args, "path", ".", base_path) {
        Ok(value) => value,
        Err(err) => return err,
    };

    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_depth = args.get("maxDepth").and_then(Value::as_u64).unwrap_or(2) as usize;

    if target.target.is_file() {
        return match fs::metadata(&target.target) {
            Ok(meta) => ToolExecutionResult::success(format!(
                "FILE {} ({} bytes)",
                target.rel_path,
                meta.len()
            )),
            Err(e) => ToolExecutionResult::error(format!("Error reading metadata: {}", e)),
        };
    }

    let paths = match collect_target_paths(base_path, &target, recursive, max_depth) {
        Ok(paths) => paths,
        Err(err) => {
            return ToolExecutionResult::error(
                err.output
                    .replace("Error scanning path:", "Error listing path:"),
            );
        }
    };

    if paths.is_empty() {
        ToolExecutionResult::success(format!("Directory {} is empty.", target.rel_path))
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
    let name = match require_non_empty_string_arg(args, "name") {
        Ok(value) => value,
        Err(err) => return err,
    };
    let target = match resolve_optional_target_path(args, "path", ".", base_path) {
        Ok(value) => value,
        Err(err) => return err,
    };

    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let case_sensitive = args
        .get("caseSensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_matches = args.get("maxMatches").and_then(Value::as_u64).unwrap_or(50) as usize;
    let max_depth = args.get("maxDepth").and_then(Value::as_u64).unwrap_or(8) as usize;

    let paths = match collect_target_paths(base_path, &target, recursive, max_depth) {
        Ok(paths) => paths,
        Err(err) => return err,
    };

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
        ToolExecutionResult::success(format!(
            "No paths matching `{}` under {}.",
            name, target.rel_path
        ))
    } else {
        ToolExecutionResult::success(matches.join("\n"))
    }
}

pub(crate) fn run_grep_tool(args: &Value, base_path: &Path) -> ToolExecutionResult {
    let pattern = match require_non_empty_string_arg(args, "pattern") {
        Ok(value) => value,
        Err(err) => return err,
    };
    let target = match resolve_optional_target_path(args, "path", ".", base_path) {
        Ok(value) => value,
        Err(err) => return err,
    };

    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let case_sensitive = args
        .get("caseSensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_matches = args.get("maxMatches").and_then(Value::as_u64).unwrap_or(50) as usize;

    let paths = match collect_target_paths(base_path, &target, recursive, usize::MAX) {
        Ok(paths) => paths,
        Err(err) => return err,
    };

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
        ToolExecutionResult::success(format!(
            "No matches for `{}` under {}.",
            pattern, target.rel_path
        ))
    } else {
        ToolExecutionResult::success(matches.join("\n"))
    }
}

pub(crate) fn core_tool_definitions() -> Vec<Value> {
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
    let command = match require_non_empty_string_arg(args, "command") {
        Ok(value) => value,
        Err(err) => return err,
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

fn run_read_tool(args: &Value, base_path: &Path) -> ToolExecutionResult {
    let rel_path = match require_safe_rel_path(args, "path", base_path) {
        Ok(path) => path,
        Err(err) => return err,
    };

    let offset = args["offset"].as_u64().unwrap_or(1) as usize;
    let limit = args["limit"].as_u64().unwrap_or(800) as usize;
    if offset == 0 {
        return ToolExecutionResult::error("Error: `offset` must be >= 1.");
    }

    let file_path = base_path.join(rel_path);
    if !file_path.exists() {
        return ToolExecutionResult::error(format!("Error: File not found: {}", rel_path));
    }

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
        Err(error) => ToolExecutionResult::error(format!("Error reading file: {}", error)),
    }
}

fn run_write_tool(args: &Value, base_path: &Path) -> ToolExecutionResult {
    let rel_path = match require_safe_rel_path(args, "path", base_path) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let content = match require_string_arg(args, "content") {
        Ok(content) => content,
        Err(err) => return err,
    };
    let full_path = base_path.join(rel_path);

    if let Some(parent) = full_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match fs::write(&full_path, content) {
        Ok(_) => ToolExecutionResult::success(format!("Successfully wrote to {}", rel_path)),
        Err(error) => ToolExecutionResult::error(format!("Error writing file: {}", error)),
    }
}

fn run_edit_tool(args: &Value, base_path: &Path) -> ToolExecutionResult {
    let rel_path = match require_safe_rel_path(args, "path", base_path) {
        Ok(path) => path,
        Err(err) => return err,
    };
    let old_text = match require_non_empty_string_arg(args, "oldText") {
        Ok(content) => content,
        Err(err) => return err,
    };
    let new_text = match require_string_arg(args, "newText") {
        Ok(content) => content,
        Err(err) => return err,
    };
    let file_path = base_path.join(rel_path);

    match fs::read_to_string(&file_path) {
        Ok(content) => {
            let occurrences: Vec<_> = content.matches(old_text).collect();
            if occurrences.len() == 1 {
                let new_content = content.replace(old_text, new_text);
                match fs::write(&file_path, new_content) {
                    Ok(_) => {
                        ToolExecutionResult::success(format!("Successfully edited {}", rel_path))
                    }
                    Err(error) => {
                        ToolExecutionResult::error(format!("Error writing file: {}", error))
                    }
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
        Err(_) => ToolExecutionResult::error(format!("Error: File not found: {}", rel_path)),
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

fn routing_tool_name(definition: &Value) -> Option<String> {
    definition
        .get("name")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn reserved_tool_names() -> HashSet<String> {
    let mut names = HashSet::new();
    for definition in core_tool_definitions()
        .into_iter()
        .chain(delivery::delivery_tool_definitions())
    {
        if let Some(name) = routing_tool_name(&definition) {
            names.insert(name);
        }
    }
    names
}

async fn dispatch_skill_tool(
    name: &str,
    args: &Value,
    base_path: &Path,
    config: &Config,
) -> Option<ToolExecutionResult> {
    let mut selected: Option<(String, skills::SkillTool, PathBuf)> = None;

    for (meta, dir) in SkillMetadata::discover_skills(base_path) {
        if let Some(tool) = meta.tools.get(name).cloned() {
            if let Some((existing_skill, _, _)) = &selected {
                return Some(ToolExecutionResult::error(format!(
                    "Error: Tool `{}` is ambiguous across multiple skills ({} and {}). Rename one of the tools.",
                    name, existing_skill, meta.name
                )));
            }

            selected = Some((meta.name, tool, dir));
        }
    }

    let (_, tool, dir) = selected?;
    let result = match skills::execute_skill_tool(&tool, &dir, base_path, args, config).await {
        Ok(output) => ToolExecutionResult::success(output),
        Err(error) => {
            ToolExecutionResult::error(format!("Error executing skill tool `{}`: {}", name, error))
        }
    };
    Some(result)
}

async fn dispatch_extension_tool(
    name: &str,
    args: &Value,
    base_path: &Path,
    config: &Config,
    channel_id: &str,
) -> ToolExecutionResult {
    if let Some(result) =
        delivery::dispatch_delivery_tool(name, args, base_path, config, channel_id).await
    {
        return result;
    }

    if let Some(result) = dispatch_skill_tool(name, args, base_path, config).await {
        return result;
    }

    ToolExecutionResult::error(format!("Error: Unknown tool `{}`", name))
}

fn dispatch_core_sync_tool(
    name: &str,
    args: &Value,
    base_path: &Path,
) -> Option<ToolExecutionResult> {
    let result = match name {
        "ls" => run_ls_tool(args, base_path),
        "find" => run_find_tool(args, base_path),
        "grep" => run_grep_tool(args, base_path),
        "read" => run_read_tool(args, base_path),
        "write" => run_write_tool(args, base_path),
        "edit" => run_edit_tool(args, base_path),
        _ => return None,
    };

    Some(result)
}

async fn dispatch_builtin_tool(
    name: &str,
    args: &Value,
    base_path: &Path,
    config: &Config,
) -> Option<ToolExecutionResult> {
    if let Some(result) = dispatch_core_sync_tool(name, args, base_path) {
        return Some(result);
    }

    if name == "exec" {
        return Some(run_exec_tool(args, base_path, config).await);
    }

    None
}

pub(crate) async fn dispatch_tool(
    name: &str,
    args: &Value,
    base_path: &Path,
    config: &Config,
    channel_id: &str,
) -> ToolExecutionResult {
    let output = match dispatch_builtin_tool(name, args, base_path, config).await {
        Some(result) => result,
        None => dispatch_extension_tool(name, args, base_path, config, channel_id).await,
    };

    output.with_truncated_output(config.runtime.max_tool_output_bytes)
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

fn extend_tool_definitions(target: &mut Vec<Value>, tools: impl IntoIterator<Item = Value>) {
    target.extend(tools);
}

fn skill_routing_tool_definitions(base_path: &Path) -> Vec<Value> {
    let reserved = reserved_tool_names();
    let discovered = SkillMetadata::discover_skills(base_path);
    let mut name_counts = HashMap::new();

    for (meta, _) in &discovered {
        for tool_name in meta.tools.keys() {
            *name_counts.entry(tool_name.clone()).or_insert(0usize) += 1;
        }
    }

    let mut tools = Vec::new();
    for (meta, _) in discovered {
        for (tool_name, tool_info) in meta.tools {
            if reserved.contains(&tool_name) {
                continue;
            }
            if name_counts.get(&tool_name).copied().unwrap_or(0) > 1 {
                continue;
            }
            tools.push(json!({
                "name": tool_name,
                "description": format!("{}: {}", meta.name, tool_info.description),
                "parameters": tool_info.parameters
            }));
        }
    }

    tools
}

pub(crate) fn get_routing_tool_definitions(base_path: &Path) -> Value {
    let mut tools = core_tool_definitions();
    extend_tool_definitions(&mut tools, delivery::delivery_tool_definitions());
    extend_tool_definitions(&mut tools, skill_routing_tool_definitions(base_path));
    json!(tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_test_skill(base_path: &Path, dir_name: &str, skill_name: &str, tool_name: &str) {
        let skill_dir = base_path.join("skills").join(dir_name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.json"),
            json!({
                "name": skill_name,
                "description": format!("{} description", skill_name),
                "guidance": format!("{} guidance", skill_name),
                "tools": [{
                    "name": tool_name,
                    "description": format!("{} tool", tool_name),
                    "parameters": { "type": "object" },
                    "command": "printf skill-ok"
                }]
            })
            .to_string(),
        )
        .unwrap();
    }

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
        }
    }

    #[tokio::test]
    async fn test_exec_tool_rejects_when_privileged_mode_is_disabled() {
        let dir = tempdir().unwrap();
        let result = dispatch_tool(
            "exec",
            &json!({ "command": "pwd" }),
            dir.path(),
            &test_config(),
            "0",
        )
        .await;

        assert!(result.is_error);
        assert!(result.output.contains("runtime.privileged=false"));
    }

    #[tokio::test]
    async fn test_exec_tool_runs_when_privileged_mode_is_enabled() {
        let dir = tempdir().unwrap();
        let mut config = test_config();
        config.runtime.privileged = true;
        let result = dispatch_tool(
            "exec",
            &json!({ "command": "printf host-ok" }),
            dir.path(),
            &config,
            "0",
        )
        .await;

        assert!(!result.is_error);
        assert_eq!(result.output, "host-ok");
    }

    #[tokio::test]
    async fn test_dispatch_tool_rejects_missing_write_content() {
        let dir = tempdir().unwrap();
        let result = dispatch_tool(
            "write",
            &json!({ "path": "notes.txt" }),
            dir.path(),
            &test_config(),
            "0",
        )
        .await;

        assert!(result.is_error);
        assert!(
            result
                .output
                .contains("Missing required argument `content`")
        );
        assert!(!dir.path().join("notes.txt").exists());
    }

    #[test]
    fn test_find_ls_and_grep_tools_work_without_shell() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::write(
            dir.path().join("docs").join("alpha.txt"),
            "hello\nfind me\n",
        )
        .unwrap();
        fs::write(dir.path().join("docs").join("beta.txt"), "nothing\n").unwrap();

        let find_result = run_find_tool(&json!({ "name": "alpha", "path": "docs" }), dir.path());
        assert!(!find_result.is_error);
        assert!(find_result.output.contains("FILE docs/alpha.txt"));

        let ls_result = run_ls_tool(&json!({ "path": "docs", "recursive": true }), dir.path());
        assert!(!ls_result.is_error);
        assert!(ls_result.output.contains("FILE docs/alpha.txt"));

        let grep_result =
            run_grep_tool(&json!({ "pattern": "find me", "path": "docs" }), dir.path());
        assert!(!grep_result.is_error);
        assert!(grep_result.output.contains("docs/alpha.txt:2: find me"));
    }

    #[test]
    fn test_read_only_tools_reject_missing_target_path() {
        let dir = tempdir().unwrap();

        let ls_result = run_ls_tool(&json!({ "path": "missing" }), dir.path());
        assert!(ls_result.is_error);
        assert!(ls_result.output.contains("Path not found: missing"));

        let find_result = run_find_tool(&json!({ "name": "alpha", "path": "missing" }), dir.path());
        assert!(find_result.is_error);
        assert!(find_result.output.contains("Path not found: missing"));

        let grep_result = run_grep_tool(
            &json!({ "pattern": "alpha", "path": "missing" }),
            dir.path(),
        );
        assert!(grep_result.is_error);
        assert!(grep_result.output.contains("Path not found: missing"));
    }

    #[test]
    fn test_read_tool_rejects_offset_beyond_file_length() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "line1\nline2\n").unwrap();

        let result = run_read_tool(&json!({ "path": "notes.txt", "offset": 3 }), dir.path());
        assert!(result.is_error);
        assert!(result.output.contains("offset 3 is beyond file length 2"));
    }

    #[test]
    fn test_edit_tool_rejects_non_unique_match() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "same\nsame\n").unwrap();

        let result = run_edit_tool(
            &json!({
                "path": "notes.txt",
                "oldText": "same",
                "newText": "changed"
            }),
            dir.path(),
        );

        assert!(result.is_error);
        assert!(result.output.contains("oldText is not unique in notes.txt"));
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

    #[test]
    fn test_routing_tool_definitions_skip_reserved_and_ambiguous_skill_tools() {
        let dir = tempdir().unwrap();
        write_test_skill(dir.path(), "reserved-skill", "ReservedSkill", "read");
        write_test_skill(dir.path(), "dup-one", "DupOne", "shared_tool");
        write_test_skill(dir.path(), "dup-two", "DupTwo", "shared_tool");
        write_test_skill(dir.path(), "unique-skill", "UniqueSkill", "unique_tool");

        let definitions = get_routing_tool_definitions(dir.path());
        let names = definitions
            .as_array()
            .unwrap()
            .iter()
            .filter_map(routing_tool_name)
            .collect::<HashSet<_>>();

        assert!(names.contains("read"));
        assert!(!names.contains("shared_tool"));
        assert!(names.contains("send_message"));
        assert!(names.contains("unique_tool"));
    }

    #[tokio::test]
    async fn test_dispatch_tool_rejects_ambiguous_skill_tool_name() {
        let dir = tempdir().unwrap();
        write_test_skill(dir.path(), "dup-one", "DupOne", "shared_tool");
        write_test_skill(dir.path(), "dup-two", "DupTwo", "shared_tool");

        let result =
            dispatch_tool("shared_tool", &json!({}), dir.path(), &test_config(), "0").await;

        assert!(result.is_error);
        assert!(result.output.contains("ambiguous across multiple skills"));
    }
}
