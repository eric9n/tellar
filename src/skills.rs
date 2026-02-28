/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/skills.rs
 * Responsibility: Discover and execute user-installed, trusted external skills.
 */

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use serde_json::Value;
use std::time::Duration;

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

impl SkillMetadata {
    pub fn from_installed_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)?;
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
        let content = fs::read_to_string(path)?;
        
        // Very basic Markdown Frontmatter parser
        if !content.starts_with("---") {
            return Err(anyhow!("Missing YAML frontmatter in SKILL.md"));
        }
        
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Err(anyhow!("Invalid SKILL.md format"));
        }
        
        let mut meta: SkillMetadata = serde_yaml::from_str(parts[1])?;
        meta.guidance = parts[2].trim().to_string();
        
        // If name is missing, use directory name
        if meta.name.is_empty() {
            meta.name = path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
        }
        
        Ok(meta)
    }

    pub fn discover_skills(base_path: &Path) -> Vec<(SkillMetadata, PathBuf)> {
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
                        match Self::from_installed_file(&installed_file) {
                            Ok(meta) => {
                                skills.push((meta, path));
                                continue;
                            }
                            Err(e) => {
                                eprintln!(
                                    "⚠️ Failed to load installed skill metadata at {}: {}",
                                    installed_file.display(),
                                    e
                                );
                            }
                        }
                    }

                    let skill_md = path.join("SKILL.md");
                    if skill_md.exists() {
                        match Self::from_file(&skill_md) {
                            Ok(meta) => {
                                eprintln!(
                                    "⚠️ Found legacy SKILL.md without SKILL.json at {}. Run `tellarctl install-skill {}` to compile it.",
                                    skill_md.display(),
                                    path.display()
                                );
                                skills.push((meta, path));
                            }
                            Err(e) => {
                                eprintln!("⚠️ Failed to load legacy skill at {}: {}", skill_md.display(), e);
                            }
                        }
                    }
                }
            }
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

pub async fn execute_skill_tool(
    tool: &SkillTool,
    skill_dir: &Path,
    workspace_dir: &Path,
    args: &Value,
    config: &crate::config::Config,
) -> Result<String> {
    let command_line = tool.shell.trim();
    if command_line.is_empty() {
        return Err(anyhow!("Empty execution line in skill tool"));
    }

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-lc").arg(command_line);

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
    if !stdout.is_empty() { result.push_str(&stdout); }
    if !stderr.is_empty() { 
        if !result.is_empty() { result.push('\n'); }
        result.push_str(&format!("STDERR:\n{}", stderr)); 
    }

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        return Err(anyhow!("Skill tool failed with exit code {}:\n{}", code, result));
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
        fs::write(
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
        fs::write(
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
            guardian: None,
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
        fs::write(
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

        let guidance = build_relevant_skill_guidance(guild.path(), "please use sample here").unwrap();
        assert!(guidance.contains("Skill: sample"));
        assert!(guidance.contains("sample operations"));
    }
}
