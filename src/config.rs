/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/config.rs
 * Responsibility: YAML configuration structure and loading
 */
use serde::{Deserialize, Serialize};
use std::fs;
use anyhow::{Context, Result};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub gemini: GeminiConfig,
    pub discord: DiscordConfig,
    pub guardian: Option<GuardianConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GuardianConfig {
    pub model: Option<String>,
}


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GeminiConfig {
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DiscordConfig {
    pub token: String,
    pub guild_id: Option<String>,
    pub channel_mappings: Option<std::collections::HashMap<String, String>>, // Discord Channel ID -> Tellar Folder Name
}

use std::path::Path;

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read config file at {:?}", path.as_ref()))?;
        let config: Config = serde_yaml::from_str(&content)
            .context("Failed to parse config file")?;
        Ok(config)
    }
}
