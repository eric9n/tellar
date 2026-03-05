use tellar::discord;
use tellar::rhythm;
use tellar::watch;

use tellar::StewardNotification;
use tellar::config::Config;

use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Parser, Debug)]
#[command(author, version, about = "Tellar - Minimal Document-Driven Cyber Steward", long_about = None)]
struct Cli {
    /// 盟友会馆 (Guild) 目录 (默认: ~/.tellar)
    #[arg(short, long, global = true)]
    guild: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!(
        r#"
    __________  ____    __    ___    ____ 
   /_  __/ __ \/ / /   /   |  / __ \/ __ \
    / / / / / / / /   / /| | / /_/ / / / /
   / / / /_/ / / /___/ ___ |/ _, _/ /_/ / 
  /_/  \____/_/_____/_/  |_/_/ |_|\____/  
    "#
    );

    let args = Cli::parse();
    let guild_path = args.guild.unwrap_or_else(tellar::default_guild_path);

    // 1. Strict check: Guild must exist (no auto-init)
    if !guild_path.exists() {
        eprintln!("❌ Guild directory not found at: {:?}", guild_path);
        eprintln!("💡 Please run 'tellarctl setup' first to initialize your Cyber Steward.");
        std::process::exit(1);
    }

    // 2. Load configuration
    let config_file = guild_path.join("tellar.yml");
    if !config_file.exists() {
        eprintln!("❌ Configuration file not found at: {:?}", config_file);
        eprintln!("💡 Please run 'tellarctl setup' to configure your API keys.");
        std::process::exit(1);
    }
    let config = Arc::new(Config::load(&config_file)?);

    // 3. Start Steward
    println!("🌳 Guild: {}", guild_path.display());
    println!("🕯️  Waking up the Cyber Steward...");
    println!("Guild foundation: {:?}", guild_path);
    println!("📖 Configuration loaded successfully!");

    // 4. Mirror Guild structure
    let shared_mappings = Arc::new(RwLock::new(HashMap::new()));
    if let Some(guild_id) = &config.discord.guild_id {
        println!("🔍 Discovering channels for Guild: {}...", guild_id);
        match discord::fetch_guild_channels(&config.discord.token, guild_id).await {
            Ok(channels) => {
                tellar::mirror_guild_structure(&guild_path, &channels)?;
                let mut map = shared_mappings.write().await;
                for (id, name) in channels {
                    map.insert(id, name.clone());
                }
            }
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

    let config_discord = Arc::clone(&config);
    let guild_discord = guild_path.clone();
    let mappings_listener = shared_mappings.clone();
    let notif_tx_discord = notif_tx.clone();

    tokio::spawn(async move {
        if let Err(e) = discord::start_listening(
            &config_discord.discord.token,
            guild_discord,
            mappings_listener,
            notif_tx_discord,
        )
        .await
        {
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
    if let Err(e) =
        discord::sync_all_discord_events(&guild_path, Some(shared_mappings.clone())).await
    {
        eprintln!("⚠️ Initial Discord event sync failed: {:?}", e);
    }

    // 7. [Orchestration Layer] Mount The Watchman
    let base_path_watch = guild_path.clone();
    let config_watch = Arc::clone(&config);

    // Keep a clone of the transmitter alive so the receiver doesn't close if Discord fails
    let _tx_keepalive = notif_tx.clone();

    // Watchman is the main synchronous orchestrator now
    if let Err(e) = watch::start_watchman(
        &base_path_watch,
        config_watch,
        notif_rx,
        shared_mappings.clone(),
    )
    .await
    {
        eprintln!("⚠️ The Watchman has fallen: {:?}", e);
    }

    Ok(())
}
