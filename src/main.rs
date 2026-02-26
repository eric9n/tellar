use tellar::discord;
use tellar::init;

use tellar::config::Config;
use tellar::StewardNotification;

use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use clap::Parser;
use std::path::PathBuf;



#[derive(Parser, Debug)]
#[command(author, version, about = "Tellar - 极简文档驱动型赛博管家", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// 盟友会馆 (Guild) 目录 (默认: ~/.tellar)
    #[arg(short, long, global = true)]
    guild: Option<PathBuf>,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Start the steward in reactive blackboard mode (default)
    Run,
    /// Interactive setup for API keys and systemd
    Setup,
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

    let args = Cli::parse();
    let guild_path = init::resolve_guild_path(args.guild);
    
    // 1. Initialize foundations (always ensures folders exist)
    init::initialize_guild(&guild_path)?;
    init::persist_guild_path(&guild_path)?;

    // 2. Load configuration
    let config_file = guild_path.join("tellar.yml");
    let mut config = Config::load(&config_file)?;

    // 3. Command Handling
    match args.command.unwrap_or(Commands::Run) {
        Commands::Setup => {
            init::run_interactive_setup(&guild_path, &mut config).await?;
            return Ok(());
        }
        Commands::Run => {
            // Check for placeholders and prompt ONLY if running interactively
            if (config.gemini.api_key.contains("YOUR_") || config.discord.token.contains("YOUR_")) 
               && atty::is(atty::Stream::Stdin) {
                println!("✨ Placeholder configuration detected. Entering setup...");
                init::run_interactive_setup(&guild_path, &mut config).await?;
            }
        }
    }

    println!("🚀 Tellar engine is cold-starting into Reactive Blackboard mode...");
    println!("Guild foundation: {:?}", guild_path);
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
    if let Err(e) = tellar::watch::start_watchman(&base_path_watch, &config_watch, notif_rx, shared_mappings.clone()).await {
        eprintln!("⚠️ The Watchman has fallen: {:?}", e);
    }


    Ok(())
}