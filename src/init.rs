/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/init.rs
 * Responsibility: System initialization and workspace management
 */

use std::fs;
use std::path::{Path, PathBuf};
// Removed Result import as it is unused
use dirs::home_dir;

/// Resolve guild path
/// Priority: CLI > Default (~/.tellar/guild)
pub fn resolve_guild_path(cli_guild: Option<PathBuf>) -> PathBuf {
    // 1. CLI override
    if let Some(path) = cli_guild {
        return path;
    }

    // 2. Sole Default path: ~/.tellar/guild
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
