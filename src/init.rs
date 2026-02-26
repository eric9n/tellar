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

fn mask_secret(secret: &str) -> String {
    if secret.len() <= 5 {
        "***".to_string()
    } else {
        format!("***{}", &secret[secret.len() - 5..])
    }
}

/// Run interactive setup for keys and systemd
pub async fn run_interactive_setup(base_path: &Path, config: &mut crate::config::Config) -> Result<()> {
    use std::io::Write;
    
    println!("\n✨ Tellar Setup - Initializing your Cyber Steward...");

    // 1. Gemini API Key
    let env_key = std::env::var("GEMINI_API_KEY").ok();
    if let Some(key) = &env_key {
        println!("\n🔑 Gemini API Key detected in environment: {}", mask_secret(key));
        println!("Press Enter to use it, or paste a new one:");
    } else {
        println!("\n🔑 Please enter your Gemini API Key:");
    }
    
    print!("> ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    
    if !trimmed.is_empty() {
        config.gemini.api_key = trimmed.to_string();
    } else if let Some(key) = env_key {
        config.gemini.api_key = key;
    } else if config.gemini.api_key.contains("YOUR_") {
        anyhow::bail!("API Key cannot be empty.");
    }

    // 2. Discord Bot Token
    let env_token = std::env::var("DISCORD_BOT_TOKEN").ok();
    if let Some(token) = &env_token {
        println!("\n🤖 Discord Bot Token detected in environment: {}", mask_secret(token));
        println!("Press Enter to use it, or paste a new one:");
    } else {
        println!("\n🤖 Please enter your Discord Bot Token:");
    }

    print!("> ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    
    if !trimmed.is_empty() {
        config.discord.token = trimmed.to_string();
    } else if let Some(token) = env_token {
        config.discord.token = token;
    } else if config.discord.token.contains("YOUR_") {
        anyhow::bail!("Discord Token cannot be empty.");
    }

    // 3. Gemini Model Selection
    println!("\n🔍 Fetching available models...");
    match crate::llm::list_models(&config.gemini.api_key).await {
        Ok(models) => {
            if !models.is_empty() {
                println!("\n🤖 Select a Cyber Brain for your Steward:");
                for (i, m) in models.iter().enumerate() {
                    println!("  [{}] {}", i + 1, m);
                }
                print!("Choice (1-{}, default 1): ", models.len());
                std::io::stdout().flush()?;
                
                let mut choice = String::new();
                std::io::stdin().read_line(&mut choice)?;
                let idx = choice.trim().parse::<usize>().unwrap_or(1).saturating_sub(1);
                config.gemini.model = models.get(idx).unwrap_or(&models[0]).clone();
                println!("✅ Brain selected: {}", config.gemini.model);
            }
        },
        Err(e) => eprintln!("⚠️ Failed to fetch models: {}. Using default.", e),
    }

    // 4. Systemd Path Replacement (Optional)
    println!("\n⚙️  Do you want to update the systemd service file with current paths? (y/N)");
    print!("> ");
    std::io::stdout().flush()?;
    let mut choice = String::new();
    std::io::stdin().read_line(&mut choice)?;
    if choice.trim().to_lowercase() == "y" {
        let abs_base_path = fs::canonicalize(base_path).unwrap_or_else(|_| base_path.to_path_buf());
        let binary_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tellar"));
        
        let service_template = abs_base_path.join("scripts").join("tellar.service");
        if service_template.exists() {
            let content = fs::read_to_string(&service_template)?;
            let updated = content
                .replace("{{GUILD_PATH}}", &abs_base_path.to_string_lossy())
                .replace("{{BINARY_PATH}}", &binary_path.to_string_lossy());
            fs::write(&service_template, updated)?;
            println!("✅ Updated scripts/tellar.service:");
            println!("   - Guild: {}", abs_base_path.display());
            println!("   - Binary: {}", binary_path.display());
        } else {
            println!("⚠️ scripts/tellar.service template not found at {:?}", service_template);
        }
    }

    // 5. Persist updated config
    let config_file = base_path.join("tellar.yml");
    let updated_yaml = serde_yaml::to_string(&config)?;
    fs::write(&config_file, updated_yaml)?;
    println!("\n📝 Configuration inscribed to tellar.yml!");

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
