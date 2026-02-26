/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/lib.rs
 * Responsibility: Shared library modules
 */

pub mod config;
pub mod llm;
pub mod steward;
pub mod rhythm;
pub mod discord;
pub mod guardian;
pub mod skills;
pub mod watch;

use std::path::{Path, PathBuf};
use std::fs;
use dirs::home_dir;

/// Returns the absolute default guild path: ~/.tellar/guild
pub fn default_guild_path() -> PathBuf {
    home_dir()
        .expect("Could not locate home directory")
        .join(".tellar")
        .join("guild")
}

/// Create local channel folders based on Discord guild discovery
pub fn mirror_guild_structure(base_path: &Path, channels: &std::collections::HashMap<String, String>) -> anyhow::Result<()> {
    for name in channels.values() {
        let channel_path = base_path.join("channels").join(name);
        
        if !channel_path.exists() {
            let _ = fs::create_dir_all(&channel_path);
            println!("📂 Synchronized new channel folder: #{}", name);
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct StewardNotification {
    pub blackboard_path: PathBuf,
    pub channel_id: String,
    pub guild_id: String,
    pub message_id: String,
    pub content: String,
}
