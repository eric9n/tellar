/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/skills.rs
 * Responsibility: Discover and execute user-installed, trusted external skills.
 */

use anyhow::{Result, anyhow};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Duration;
use std::time::SystemTime;

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub tools: HashMap<String, SkillTool>,
    #[serde(skip)]
    pub guidance: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SkillTool {
    pub description: String,
    pub shell: String, // The command or script to run
    pub parameters: Value,
}

#[derive(Debug, Deserialize)]
struct InstalledSkill {
    name: String,
    description: String,
    #[serde(default)]
    guidance: Option<String>,
    tools: Vec<InstalledSkillTool>,
}

#[derive(Debug, Deserialize)]
struct InstalledSkillTool {
    name: String,
    description: String,
    parameters: Value,
    command: String,
}

const DEFAULT_SKILL_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillDiscoveryStamp {
    skills_dir_modified: Option<SystemTime>,
    entry_count: usize,
    latest_skill_file_modified: Option<SystemTime>,
}

#[derive(Clone)]
struct CachedSkillDiscovery {
    stamp: SkillDiscoveryStamp,
    skills: Vec<(SkillMetadata, PathBuf)>,
}

static SKILL_DISCOVERY_CACHE: Lazy<RwLock<HashMap<PathBuf, CachedSkillDiscovery>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

fn modified_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

pub(crate) fn skill_discovery_stamp(base_path: &Path) -> SkillDiscoveryStamp {
    let skills_dir = base_path.join("skills");
    let mut entry_count = 0;
    let mut latest_skill_file_modified = None;

    if let Ok(entries) = fs::read_dir(&skills_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            entry_count += 1;

            for candidate in [path.join("SKILL.json"), path.join("SKILL.md")] {
                if let Some(modified) = modified_time(&candidate) {
                    latest_skill_file_modified = Some(
                        latest_skill_file_modified
                            .map(|current: SystemTime| current.max(modified))
                            .unwrap_or(modified),
                    );
                }
            }
        }
    }

    SkillDiscoveryStamp {
        skills_dir_modified: modified_time(&skills_dir),
        entry_count,
        latest_skill_file_modified,
    }
}

fn discover_skills_uncached(base_path: &Path) -> Vec<(SkillMetadata, PathBuf)> {
    let mut skills = Vec::new();
    let skills_dir = base_path.join("skills");

    if !skills_dir.exists() {
        return skills;
    }

    if let Ok(entries) = fs::read_dir(skills_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let installed_file = path.join("SKILL.json");
                if installed_file.exists() {
                    match SkillMetadata::from_installed_file(&installed_file) {
                        Ok(meta) => {
                            skills.push((meta, path));
                            continue;
                        }
                        Err(e) => {
                            eprintln!(
                                "⚠️ Failed to load cached SKILL.json at {}: {}. Falling back to SKILL.md.",
                                installed_file.display(),
                                e
                            );
                        }
                    }
                }

                let skill_md = path.join("SKILL.md");
                if skill_md.exists() {
                    match SkillMetadata::from_file(&skill_md) {
                        Ok(meta) => {
                            skills.push((meta, path));
                        }
                        Err(e) => {
                            eprintln!(
                                "⚠️ Failed to load legacy skill at {}: {}",
                                skill_md.display(),
                                e
                            );
                        }
                    }
                }
            }
        }
    }

    skills
}

impl SkillMetadata {
    pub fn from_installed_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let installed: InstalledSkill = serde_json::from_str(&content)?;

        let mut tools = HashMap::new();
        for tool in installed.tools {
            tools.insert(
                tool.name,
                SkillTool {
                    description: tool.description,
                    shell: tool.command,
                    parameters: tool.parameters,
                },
            );
        }

        Ok(Self {
            name: installed.name,
            tools,
            guidance: installed.guidance.unwrap_or(installed.description),
        })
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;

        // Very basic Markdown Frontmatter parser
        if !content.starts_with("---") {
            return Err(anyhow!("Missing YAML frontmatter in SKILL.md"));
        }

        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Err(anyhow!("Invalid SKILL.md format"));
        }

        let mut meta: SkillMetadata = serde_yml::from_str(parts[1])?;
        meta.guidance = parts[2].trim().to_string();

        // If name is missing, use directory name
        if meta.name.is_empty() {
            meta.name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
        }

        Ok(meta)
    }

    pub fn discover_skills(base_path: &Path) -> Vec<(SkillMetadata, PathBuf)> {
        let cache_key = base_path.to_path_buf();
        let stamp = skill_discovery_stamp(base_path);

        if let Some(cached) = SKILL_DISCOVERY_CACHE
            .read()
            .ok()
            .and_then(|cache| cache.get(&cache_key).cloned())
            && cached.stamp == stamp {
                return cached.skills;
            }

        let skills = discover_skills_uncached(base_path);
        if let Ok(mut cache) = SKILL_DISCOVERY_CACHE.write() {
            cache.insert(
                cache_key,
                CachedSkillDiscovery {
                    stamp,
                    skills: skills.clone(),
                },
            );
        }
        skills
    }
}

pub fn build_relevant_skill_guidance(base_path: &Path, text: &str) -> Option<String> {
    let normalized = text.to_ascii_lowercase();
    let mut blocks = Vec::new();

    for (meta, _) in SkillMetadata::discover_skills(base_path) {
        if meta.guidance.is_empty() {
            continue;
        }

        let skill_match = normalized.contains(&meta.name.to_ascii_lowercase());
        let tool_match = meta
            .tools
            .keys()
            .any(|tool_name| normalized.contains(&tool_name.to_ascii_lowercase()));

        if skill_match || tool_match {
            blocks.push(format!("### Skill: {}\n{}", meta.name, meta.guidance));
        }
    }

    if blocks.is_empty() {
        None
    } else {
        Some(format!(
            "### Relevant Skill Guidance\n{}\nOnly use this guidance for the matching skill(s) above.",
            blocks.join("\n\n")
        ))
    }
}

fn matches_named_reference(haystack: &str, needle: &str) -> bool {
    if haystack.contains(needle) {
        return true;
    }

    let dash = needle.replace('_', "-");
    if dash != needle && haystack.contains(&dash) {
        return true;
    }

    let underscore = needle.replace('-', "_");
    if underscore != needle && haystack.contains(&underscore) {
        return true;
    }

    false
}

pub fn has_explicit_skill_match(base_path: &Path, text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();

    SkillMetadata::discover_skills(base_path)
        .into_iter()
        .any(|(meta, _)| {
            matches_named_reference(&normalized, &meta.name.to_ascii_lowercase())
                || meta.tools.keys().any(|tool_name| {
                    matches_named_reference(&normalized, &tool_name.to_ascii_lowercase())
                })
        })
}

pub fn find_explicit_tool_match(base_path: &Path, text: &str) -> Option<String> {
    let normalized = text.to_ascii_lowercase();

    SkillMetadata::discover_skills(base_path)
        .into_iter()
        .flat_map(|(meta, _)| meta.tools.into_keys())
        .find(|tool_name| matches_named_reference(&normalized, &tool_name.to_ascii_lowercase()))
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    let escaped = value.replace('\'', r#"'\''"#);
    format!("'{}'", escaped)
}

fn lookup_path<'a>(root: &'a Value, current: &'a Value, key: &str) -> Option<&'a Value> {
    if key == "." {
        return Some(current);
    }

    let resolve_from = |base: &'a Value, path: &str| -> Option<&'a Value> {
        let mut value = base;
        for segment in path.split('.') {
            value = value.get(segment)?;
        }
        Some(value)
    };

    resolve_from(current, key).or_else(|| resolve_from(root, key))
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(_) => true,
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(_) => true,
    }
}

fn template_value_to_shell(value: &Value) -> Option<String> {
    match value {
        Value::String(v) => Some(shell_quote(v)),
        Value::Number(v) => Some(shell_quote(&v.to_string())),
        Value::Bool(v) => Some(shell_quote(&v.to_string())),
        Value::Null => Some(shell_quote("")),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value)
            .ok()
            .map(|serialized| shell_quote(&serialized)),
    }
}

fn find_section_close(template: &str, name: &str, body_start: usize) -> Option<(usize, usize)> {
    let mut depth = 1usize;
    let mut search_from = body_start;
    let open_tag = format!("{{{{#{}}}}}", name);
    let inverted_tag = format!("{{{{^{}}}}}", name);
    let close_tag = format!("{{{{/{}}}}}", name);

    while let Some(rel) = template[search_from..].find("{{") {
        let tag_start = search_from + rel;
        let rest = &template[tag_start..];

        if rest.starts_with(&open_tag) || rest.starts_with(&inverted_tag) {
            depth += 1;
            search_from = tag_start + 2;
            continue;
        }

        if rest.starts_with(&close_tag) {
            depth -= 1;
            if depth == 0 {
                return Some((tag_start, tag_start + close_tag.len()));
            }
            search_from = tag_start + 2;
            continue;
        }

        search_from = tag_start + 2;
    }

    None
}

fn render_template_fragment(template: &str, root: &Value, current: &Value) -> Result<String> {
    let mut out = String::new();
    let mut pos = 0usize;

    while let Some(rel_start) = template[pos..].find("{{") {
        let start = pos + rel_start;
        out.push_str(&template[pos..start]);

        let rest = &template[start + 2..];
        let Some(rel_end) = rest.find("}}") else {
            return Err(anyhow!("Unclosed template tag in skill command template"));
        };
        let end = start + 2 + rel_end;
        let tag = rest[..rel_end].trim();
        pos = end + 2;

        if let Some(name) = tag.strip_prefix('#').map(str::trim) {
            let body_start = pos;
            let Some((close_start, close_end)) = find_section_close(template, name, body_start)
            else {
                return Err(anyhow!(
                    "Unclosed section `{}` in skill command template",
                    name
                ));
            };
            let body = &template[body_start..close_start];
            let value = lookup_path(root, current, name).unwrap_or(&Value::Null);

            match value {
                Value::Array(items) => {
                    for item in items {
                        out.push_str(&render_template_fragment(body, root, item)?);
                    }
                }
                _ if is_truthy(value) => {
                    out.push_str(&render_template_fragment(body, root, value)?);
                }
                _ => {}
            }

            pos = close_end;
            continue;
        }

        if let Some(name) = tag.strip_prefix('^').map(str::trim) {
            let body_start = pos;
            let Some((close_start, close_end)) = find_section_close(template, name, body_start)
            else {
                return Err(anyhow!(
                    "Unclosed inverted section `{}` in skill command template",
                    name
                ));
            };
            let body = &template[body_start..close_start];
            let value = lookup_path(root, current, name).unwrap_or(&Value::Null);

            if !is_truthy(value) {
                out.push_str(&render_template_fragment(body, root, current)?);
            }

            pos = close_end;
            continue;
        }

        if tag.starts_with('/') {
            return Err(anyhow!(
                "Unexpected closing tag `{}` in skill command template",
                tag
            ));
        }

        if tag.starts_with('!') {
            continue;
        }

        let value = lookup_path(root, current, tag).ok_or_else(|| {
            anyhow!(
                "Missing template value for `{}` in skill command template",
                tag
            )
        })?;
        let replacement = template_value_to_shell(value).ok_or_else(|| {
            anyhow!(
                "Unsupported template value for `{}` in skill command template",
                tag
            )
        })?;
        out.push_str(&replacement);
    }

    out.push_str(&template[pos..]);
    Ok(out)
}

fn render_simple_shell_template(shell: &str, args: &Value) -> Result<String> {
    let rendered = render_template_fragment(shell, args, args)?;

    if rendered.contains("{{") || rendered.contains("}}") {
        return Err(anyhow!(
            "Unresolved skill command template placeholders remain: `{}`",
            rendered
        ));
    }

    Ok(rendered)
}

pub async fn execute_skill_tool(
    tool: &SkillTool,
    skill_dir: &Path,
    workspace_dir: &Path,
    args: &Value,
    config: &crate::config::Config,
) -> Result<String> {
    let command_line = render_simple_shell_template(tool.shell.trim(), args)?;
    if command_line.is_empty() {
        return Err(anyhow!("Empty execution line in skill tool"));
    }

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-lc").arg(&command_line);

    let args_json = serde_json::to_string(args)?;

    // Skills run from their own directory for predictable relative paths, but they are not
    // sandboxed to that directory. User-installed skills are treated as trusted extensions.
    let output_future = cmd
        .current_dir(skill_dir)
        .env("TELLAR_ARGS", &args_json)
        .env("SKILL_DIR", skill_dir)
        .env("TELLAR_WORKSPACE", workspace_dir)
        .env("TELLAR_CORE_TOOLS", "ls,find,grep,read,write,edit")
        .env("GEMINI_API_KEY", &config.gemini.api_key)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let timeout_secs = DEFAULT_SKILL_TIMEOUT_SECS;
    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), output_future)
        .await
        .map_err(|_| {
            anyhow!(
                "Skill tool timed out after {}s: `{}`",
                timeout_secs,
                command_line
            )
        })?
        .map_err(|e| anyhow!("Failed to execute skill tool `{}`: {}", command_line, e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("STDERR:\n{}", stderr));
    }

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        return Err(anyhow!(
            "Skill tool failed with exit code {}:\n{}",
            code,
            result
        ));
    }

    if result.is_empty() {
        result = "Executed successfully with no output.".to_string();
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DiscordConfig, GeminiConfig, RuntimeConfig};
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn test_skill_metadata_parses_minimal_tool_schema() {
        let dir = tempdir().unwrap();
        let skill_md = dir.path().join("SKILL.md");
        std::fs::write(
            &skill_md,
            r#"---
name: sample
tools:
  demo:
    description: Demo tool
    shell: demo.sh
    parameters:
      type: object
---
Body
"#,
        )
        .unwrap();

        let meta = SkillMetadata::from_file(&skill_md).unwrap();
        let tool = meta.tools.get("demo").unwrap();
        assert_eq!(tool.description, "Demo tool");
        assert_eq!(tool.shell, "demo.sh");
        assert_eq!(tool.parameters["type"], "object");
        assert_eq!(meta.guidance, "Body");
    }

    #[test]
    fn test_skill_metadata_parses_installed_skill_json() {
        let dir = tempdir().unwrap();
        let skill_json = dir.path().join("SKILL.json");
        std::fs::write(
            &skill_json,
            r#"{
  "name": "sample",
  "description": "Sample skill",
  "guidance": "Use when asked for sample operations.",
  "tools": [
    {
      "name": "demo",
      "description": "Demo tool",
      "parameters": { "type": "object" },
      "command": "printf hi"
    }
  ]
}"#,
        )
        .unwrap();

        let meta = SkillMetadata::from_installed_file(&skill_json).unwrap();
        let tool = meta.tools.get("demo").unwrap();
        assert_eq!(meta.name, "sample");
        assert_eq!(meta.guidance, "Use when asked for sample operations.");
        assert_eq!(tool.description, "Demo tool");
        assert_eq!(tool.shell, "printf hi");
    }

    #[tokio::test]
    async fn test_execute_skill_tool_runs_in_skill_directory() {
        let dir = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        let tool = SkillTool {
            description: "pwd".to_string(),
            shell: "printf \"$PWD\"".to_string(),
            parameters: json!({ "type": "object" }),
        };
        let config = Config {
            gemini: GeminiConfig {
                api_key: "fake".to_string(),
                model: "fake".to_string(),
            },
            discord: DiscordConfig {
                token: "fake".to_string(),
                guild_id: None,
                channel_mappings: None,
            },
            runtime: RuntimeConfig::default(),
        };

        let output = execute_skill_tool(&tool, dir.path(), workspace.path(), &json!({}), &config)
            .await
            .unwrap();

        let expected = std::fs::canonicalize(dir.path()).unwrap();
        let actual = std::fs::canonicalize(output).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_build_relevant_skill_guidance_matches_skill_name_and_body() {
        let guild = tempdir().unwrap();
        let skill_dir = guild.path().join("skills").join("sample");
        fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: sample
tools:
  demo:
    description: Demo tool
    shell: printf hi
    parameters:
      type: object
---
Use this skill when the user asks for sample operations.
"#,
        )
        .unwrap();

        let guidance =
            build_relevant_skill_guidance(guild.path(), "please use sample here").unwrap();
        assert!(guidance.contains("Skill: sample"));
        assert!(guidance.contains("sample operations"));
    }

    #[test]
    fn test_render_simple_shell_template_replaces_scalar_placeholders() {
        let rendered = render_simple_shell_template(
            "./snapshot.sh stock-quote --symbol {{symbol}} --limit {{limit}}",
            &json!({
                "symbol": "TSLA.US",
                "limit": 5
            }),
        )
        .unwrap();

        assert_eq!(
            rendered,
            "./snapshot.sh stock-quote --symbol 'TSLA.US' --limit '5'"
        );
    }

    #[test]
    fn test_render_simple_shell_template_rejects_unresolved_placeholders() {
        let err = render_simple_shell_template(
            "./snapshot.sh stock-quote --symbol {{symbol}}",
            &json!({}),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("Missing template value for `symbol`")
        );
    }

    #[test]
    fn test_render_simple_shell_template_supports_all_json_value_types() {
        let rendered = render_simple_shell_template(
            "cmd {{str}} {{num}} {{bool}} {{nullv}} {{arr}} {{obj}}",
            &json!({
                "str": "hello",
                "num": 5,
                "bool": true,
                "nullv": null,
                "arr": [1, "x"],
                "obj": { "k": "v" }
            }),
        )
        .unwrap();

        assert_eq!(
            rendered,
            "cmd 'hello' '5' 'true' '' '[1,\"x\"]' '{\"k\":\"v\"}'"
        );
    }

    #[test]
    fn test_render_simple_shell_template_supports_boolean_sections() {
        let rendered = render_simple_shell_template(
            "cmd{{#json}} --json{{/json}}{{^json}} --no-json{{/json}}",
            &json!({ "json": true }),
        )
        .unwrap();

        assert_eq!(rendered, "cmd --json");
    }

    #[test]
    fn test_render_simple_shell_template_supports_inverted_sections() {
        let rendered = render_simple_shell_template(
            "cmd{{#json}} --json{{/json}}{{^json}} --no-json{{/json}}",
            &json!({ "json": false }),
        )
        .unwrap();

        assert_eq!(rendered, "cmd --no-json");
    }

    #[test]
    fn test_render_simple_shell_template_supports_array_sections_and_dot() {
        let rendered = render_simple_shell_template(
            "cmd{{#symbols}} --symbol {{.}}{{/symbols}}",
            &json!({ "symbols": ["TSLA.US", "QQQ.US"] }),
        )
        .unwrap();

        assert_eq!(rendered, "cmd --symbol 'TSLA.US' --symbol 'QQQ.US'");
    }
}
