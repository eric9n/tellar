/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/main.rs
 * Responsibility: System core orchestrator. Unifies the Workspace under a Reactive Blackboard architecture.
 */

mod discord;
mod llm;
mod steward;
mod rhythm;
mod init;
mod config;
mod guardian;
mod skills;
mod watch;


use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

use clap::Parser;
use std::path::PathBuf;
use crate::config::Config;
use std::io::Write;

#[derive(Debug)]
pub struct StewardNotification {
    pub blackboard_path: PathBuf,
    pub channel_id: String,
    pub guild_id: String,
    pub message_id: String,
    pub content: String,
}



#[derive(Parser, Debug)]
#[command(author, version, about = "Tellar - 极简文档驱动型赛博管家", long_about = None)]
struct Cli {
    /// 盟友会馆 (Guild) 目录 (默认: ~/.tellar)
    #[arg(short, long)]
    guild: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!(r#"
    __________  ____    __    ___    ____ 
   /_  __/ __ \/ / /   /   |  / __ \/ __ \
    / / / / / / / /   / /| | / /_/ / / / /
   / / / /_/ / / /___/ ___ |/ _, _/ /_/ / 
  /_/  \____/_/_____/_/  |_/_/ |_|\____/  
    "#);
    println!("🚀 Tellar engine is cold-starting into Reactive Blackboard mode...");

    // 1. Determine guild path
    let args = Cli::parse();
    let guild_path = init::resolve_guild_path(args.guild);
    println!("Guild foundation: {:?}", guild_path);

    // 2. Initialize foundations
    init::initialize_guild(&guild_path)?;
    init::persist_guild_path(&guild_path)?;

    // 3. Load configuration
    let config_file = guild_path.join("tellar.yml");
    let mut config = Config::load(&config_file)?;
    
    // 3.1 Interactive Setup (if placeholders detected)
    if config.gemini.api_key.contains("YOUR_") || 
       config.gemini.model.is_empty() ||
       config.discord.token.contains("YOUR_") {

        
        println!("✨ First-run detected or placeholder configuration found.");
        println!("Let's get your Tellar Steward ready in seconds!");
        
        // 3.1.1 Gemini API Key
        let mut api_key = config.gemini.api_key.clone();
        if api_key.contains("YOUR_") {

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
            
            if trimmed.is_empty() {
                if let Some(key) = env_key {
                    api_key = key;
                } else {
                    anyhow::bail!("API Key cannot be empty. Please edit tellar.yml manually.");
                }
            } else {
                api_key = trimmed.to_string();
            }
            config.gemini.api_key = api_key.clone();
        }

        // 3.1.2 Discord Bot Token
        if config.discord.token.contains("YOUR_") {
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
            
            if trimmed.is_empty() {
                if let Some(token) = env_token {
                    config.discord.token = token;
                } else {
                    anyhow::bail!("Discord Token cannot be empty. Please edit tellar.yml manually.");
                }
            } else {
                config.discord.token = trimmed.to_string();
            }
        }

        // 3.1.3 Gemini Model Selection
        println!("🔍 Fetching available models for you...");
        match llm::list_models(&api_key).await {
            Ok(models) => {
                if models.is_empty() {
                    println!("⚠️ No models found for this API Key. Sticking to default.");
                } else {
                    println!("\n🤖 Select a Cyber Brain for your Steward:");
                    for (i, m) in models.iter().enumerate() {
                        println!("  [{}] {}", i + 1, m);
                    }
                    print!("Choice (1-{}, default 1): ", models.len());
                    std::io::stdout().flush()?;
                    
                    let mut choice = String::new();
                    std::io::stdin().read_line(&mut choice)?;
                    let idx = choice.trim().parse::<usize>().unwrap_or(1).saturating_sub(1);
                    let selected_model = models.get(idx).unwrap_or(&models[0]);
                    
                    println!("✅ Brain selected: {}", selected_model);
                    
                    // Update config in memory
                    config.gemini.model = selected_model.clone();
                }
            },
            Err(e) => {
                eprintln!("⚠️ Failed to fetch models: {}. Using existing config.", e);
            }
        }

        // Persist updated config to file
        let updated_yaml = serde_yaml::to_string(&config)?;
        std::fs::write(&config_file, updated_yaml)?;
        println!("📝 Configuration inscribed to tellar.yml!");

        // Cleanup the example file after successful setup
        let example_file = guild_path.join("tellar.yml.example");
        if example_file.exists() {
            let _ = std::fs::remove_file(example_file);
        }
    }


    println!("📖 Configuration loaded successfully!");


    // 4. Mirror Guild structure
    let shared_mappings = Arc::new(RwLock::new(HashMap::new()));
    if let Some(guild_id) = &config.discord.guild_id {
        println!("🔍 Discovering channels for Guild: {}...", guild_id);
        match discord::fetch_guild_channels(&config.discord.token, guild_id).await {
            Ok(channels) => {
                init::mirror_guild_structure(&guild_path, &channels)?; // Corrected call to init::mirror_guild_structure
                let mut map = shared_mappings.write().await;
                for (id, name) in channels {
                    map.insert(id, name.clone());
                }
            },
            Err(e) => eprintln!("⚠️ Guild discovery failed: {:?}", e),
        }
    }


    if let Some(manual) = &config.discord.channel_mappings {
        let mut map = shared_mappings.write().await;
        for (id, folder) in manual {
            map.insert(id.clone(), folder.clone());
        }
    }

    // 5. [Perception Layer] Start Discord Inscriber
    let (notif_tx, notif_rx) = tokio::sync::mpsc::channel::<StewardNotification>(100);
    
    let config_discord = config.clone();
    let guild_discord = guild_path.clone();
    let mappings_listener = shared_mappings.clone();
    let notif_tx_discord = notif_tx.clone();
    
    tokio::spawn(async move {
        if let Err(e) = discord::start_listening(
            &config_discord.discord.token, 
            guild_discord, 
            mappings_listener,
            notif_tx_discord
        ).await {
            eprintln!("⚠️ Discord inscriber exited abnormally: {:?}", e);
        }
    });

    // 6. [Rhythm Layer] Start the Heartbeat of Persistent Intent
    let guild_rhythm = guild_path.clone();
    tokio::spawn(async move {
        if let Err(e) = rhythm::run_rhythm(&guild_rhythm).await {
            eprintln!("⚠️ Rhythm engine exited abnormally: {:?}", e);
        }
    });

    // Initial Discord Events Sync (Ensure existing ritual files are up to date)
    if let Err(e) = discord::sync_all_discord_events(&guild_path, Some(shared_mappings.clone())).await {
        eprintln!("⚠️ Initial Discord event sync failed: {:?}", e);
    }


    // 7. [Maintenance Layer] Mount The Guardian (Silent Observer)
    let base_path_guardian = guild_path.clone();
    let config_guardian = config.clone();
    tokio::spawn(async move {
        if let Err(e) = guardian::run_guardian_loop(base_path_guardian, config_guardian).await {
            eprintln!("⚠️ The Guardian service exited abnormally: {:?}", e);
        }
    });

    // 8. [Orchestration Layer] Mount The Watchman
    let base_path_watch = guild_path.clone();
    let config_watch = config.clone();
    
    // Keep a clone of the transmitter alive so the receiver doesn't close if Discord fails
    let _tx_keepalive = notif_tx.clone();

    // Watchman is the main synchronous orchestrator now
    if let Err(e) = watch::start_watchman(&base_path_watch, &config_watch, notif_rx, shared_mappings.clone()).await {
        eprintln!("⚠️ The Watchman has fallen: {:?}", e);
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