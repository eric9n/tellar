/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/init.rs
 * Responsibility: System initialization and workspace management
 */

use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use dirs::home_dir;
use include_dir::{include_dir, Dir};

static GUILD_ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/guild");

const GUILD_POINTER: &str = ".tellar_guild";

/// Resolve guild path
/// Priority: CLI > Environment Variable > Pointer File > Default (~/.tellar)
pub fn resolve_guild_path(cli_guild: Option<PathBuf>) -> PathBuf {
    // 1. CLI takes highest priority
    if let Some(path) = cli_guild {
        return path;
    }

    // 2. Local "guild" folder (if it exists in current dir)
    if let Ok(cwd) = std::env::current_dir() {
        let local_guild = cwd.join("guild");
        if local_guild.is_dir() {
            return local_guild;
        }
    }

    // 3. Environment variable
    if let Ok(env_path) = std::env::var("TELLAR_GUILD") {
        return PathBuf::from(env_path);
    }

    // 3. Pointer file (persistent memory)
    if let Some(home) = home_dir() {
        let pointer_path = home.join(GUILD_POINTER);
        if let Ok(persisted_path) = fs::read_to_string(&pointer_path) {
            let path = PathBuf::from(persisted_path.trim());
            // Return path if it still exists
            if path.exists() {
                return path;
            }
        }
    }

    // 4. Default path
    home_dir()
        .expect("Could not locate home directory")
        .join(".tellar")
        .join("guild")
}

/// Persist guild path
pub fn persist_guild_path(path: &Path) -> Result<()> {
    if let Some(home) = home_dir() {
        let pointer_path = home.join(GUILD_POINTER);
        fs::write(pointer_path, path.to_string_lossy().as_ref())
            .context("Failed to save guild pointer file")?;
    }
    Ok(())
}

/// Initialize guild structure
pub fn initialize_guild(base_path: &Path) -> Result<()> {
    // 1. Extract all embedded assets (this creates the directory structure)
    extract_embedded_assets(&GUILD_ASSETS, base_path)?;

    // 2. Ensure critical structural folders exist (even if template is empty)
    let critical_dirs = [
        base_path.join("brain").join("attachments"),
        base_path.join("brain").join("events"),
        base_path.join("rituals"),
    ];

    for dir in critical_dirs {
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
    }

    // 2. Handle configuration setup
    let config_path = base_path.join("tellar.yml");
    let example_path = base_path.join("tellar.yml.example");

    if !config_path.exists() && example_path.exists() {
        fs::copy(&example_path, &config_path)?;
        println!("📝 Initialized default tellar.yml from template. Please configure your tokens!");
    } else if config_path.exists() && example_path.exists() {
        // Cleanup example file if the main config is already present and active
        let _ = fs::remove_file(&example_path);
    }


    Ok(())
}

fn extract_embedded_assets(dir: &Dir, base_path: &Path) -> Result<()> {
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(d) => {
                let path = base_path.join(d.path());
                if !path.exists() {
                    fs::create_dir_all(&path)?;
                }
                extract_embedded_assets(d, base_path)?;
            }
            include_dir::DirEntry::File(f) => {
                let path = base_path.join(f.path());
                
                // Skip .gitkeep files used only for Git directory tracking
                if f.path().file_name().and_then(|s| s.to_str()) == Some(".gitkeep") {
                    continue;
                }

                if !path.exists() {

                    // Create parent directories if missing (safety)
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&path, f.contents())?;
                    println!("💎 Extracted default asset: {:?}", f.path());
                }
            }
        }
    }
    Ok(())
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
