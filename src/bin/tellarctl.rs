/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/bin/tellarctl.rs
 * Responsibility: CLI for service management (systemd)
 */

use clap::{Parser, Subcommand};
use std::fs;
use std::process::Command;
use anyhow::{Context, Result};
use dirs::home_dir;
use tellar::config::Config;
use tellar::init;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "tellarctl")]
#[command(about = "Tellar Service Manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// 盟友会馆 (Guild) 目录 (默认: ~/.tellar)
    #[arg(short, long, global = true)]
    guild: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Full interactive setup (Keys + Service)
    Setup,
    /// Start the Tellar service
    Start,
    /// Stop the Tellar service
    Stop,
    /// Restart the Tellar service
    Restart,
    /// Check the service status
    Status,
    /// View real-time logs
    Logs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let service_name = "tellar";
    
    let guild_path = init::resolve_guild_path(cli.guild);
    let home = home_dir().expect("Could not locate home directory");
    let systemd_user_dir = home.join(".config").join("systemd").join("user");
    let target_service_path = systemd_user_dir.join(format!("{}.service", service_name));

    match cli.command {
        Commands::Setup => {
            println!("🕯️  Starting comprehensive setup for Tellar...");
            
            // 1. Ensure foundations exist
            init::initialize_guild(&guild_path)?;
            init::persist_guild_path(&guild_path)?;

            // 2. Load/Create configuration
            let config_file = guild_path.join("tellar.yml");
            let mut config = Config::load(&config_file).unwrap_or_else(|_| {
                // Return a default config if not found
                serde_yaml::from_str("gemini:\n  api_key: \"YOUR_KEY\"\n  model: \"\"\ndiscord:\n  token: \"YOUR_TOKEN\"").unwrap()
            });

            // 3. Run interactive Key/Model/Systemd path setup
            init::run_interactive_setup(&guild_path, &mut config).await?;

            // 4. Finalize Systemd Service Installation
            println!("\n� Finalizing systemd service installation...");
            if !systemd_user_dir.exists() {
                fs::create_dir_all(&systemd_user_dir).context("Failed to create systemd user directory")?;
            }

            // The setup logic in init.rs already updated scripts/tellar.service
            // We just need to copy it to the systemd directory
            let service_template = guild_path.join("scripts").join("tellar.service");
            if service_template.exists() {
                fs::copy(&service_template, &target_service_path).context("Failed to copy service file")?;
                println!("✅ Service file installed at {:?}", target_service_path);
            }

            // Reload systemd
            run_cmd("systemctl", &["--user", "daemon-reload"]);
            run_cmd("systemctl", &["--user", "enable", service_name]);

            // Enable linger
            let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
            println!("🔧 Enabling linger for {}...", user);
            run_cmd("loginctl", &["enable-linger", &user]);

            println!("\n🚀 Setup complete! Your Steward is ready.");
            println!("Run 'tellarctl start' to begin.");
        }
        Commands::Start => {
            println!("🕯️ Starting Tellar...");
            run_cmd("systemctl", &["--user", "start", service_name]);
        }
        Commands::Stop => {
            println!("😴 Stopping Tellar...");
            run_cmd("systemctl", &["--user", "stop", service_name]);
        }
        Commands::Restart => {
            println!("🔄 Restarting Tellar...");
            run_cmd("systemctl", &["--user", "restart", service_name]);
        }
        Commands::Status => {
            run_cmd("systemctl", &["--user", "status", service_name]);
        }
        Commands::Logs => {
            run_cmd("journalctl", &["--user", "-u", service_name, "-f"]);
        }
    }
    Ok(())
}

fn run_cmd(cmd: &str, args: &[&str]) {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .unwrap_or_else(|_| panic!("Failed to execute {}", cmd));
    
    if !status.success() {
        eprintln!("❌ Command '{} {:?}' failed", cmd, args);
    }
}
