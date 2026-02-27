/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/skills.rs
 * Responsibility: Discover and execute external skills (scripts)
 */

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use serde_json::Value;

#[derive(Debug, Deserialize, Default, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub tools: HashMap<String, SkillTool>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SkillTool {
    pub description: String,
    pub shell: String, // The command or script to run
    pub parameters: Value,
}

impl SkillMetadata {
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
                    let skill_md = path.join("SKILL.md");
                    if skill_md.exists() {
                        if let Ok(meta) = Self::from_file(&skill_md) {
                            skills.push((meta, path));
                        }
                    }
                }
            }
        }
        skills
    }
}

pub async fn execute_skill_tool(
    script_line: &str,
    skill_dir: &Path,
    workspace_dir: &Path,
    args: &Value,
    config: &crate::config::Config,
) -> Result<String> {
    let parts: Vec<String> = script_line.split_whitespace().map(|s| s.to_string()).collect();
    if parts.is_empty() {
        return Err(anyhow!("Empty execution line in skill tool"));
    }

    let mut cmd = if parts[0].ends_with(".py") || parts[0].ends_with(".js") || parts[0].ends_with(".sh") {
        let script_path = skill_dir.join("tools").join(&parts[0]);
        let ext = script_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let interpreter = match ext {
            "py" => "python3",
            "js" => "node",
            "sh" => "bash",
            _ => "bash",
        };
        let mut c = tokio::process::Command::new(interpreter);
        c.arg(script_path);
        c
    } else {
        let mut c = tokio::process::Command::new(&parts[0]);
        if parts.len() > 1 {
            c.args(&parts[1..]);
        }
        c
    };

    let args_json = serde_json::to_string(args)?;

    let output = cmd
        .env("TELLAR_ARGS", &args_json)
        .env("SKILL_DIR", skill_dir)
        .env("TELLAR_WORKSPACE", workspace_dir)
        .env("TELLAR_CORE_TOOLS", "ls,grep,read,write,edit")
        .env("GEMINI_API_KEY", &config.gemini.api_key)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| anyhow!("Failed to execute skill tool `{}`: {}", parts[0], e))?;

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
