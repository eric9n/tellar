/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/bin/tellarctl.rs
 * Responsibility: CLI for service management (systemd)
 */

use clap::{Parser, Subcommand};
use std::fs;
use std::process::Command;
use std::path::PathBuf;
use dirs::home_dir;

#[derive(Parser)]
#[command(name = "tellarctl")]
#[command(about = "Tellar Service Manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Setup the systemd user service
    Setup {
        /// Path to the guild directory (defaults to ~/.tellar/guild)
        #[arg(short, long)]
        guild: Option<String>,
    },
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

fn main() {
    let cli = Cli::parse();
    let service_name = "tellar";
    let home = home_dir().expect("Could not locate home directory");
    let systemd_user_dir = home.join(".config").join("systemd").join("user");
    let target_service_path = systemd_user_dir.join(format!("{}.service", service_name));

    match cli.command {
        Commands::Setup { guild } => {
            let guild_path = guild.unwrap_or_else(|| {
                home.join(".tellar").join("guild").to_string_lossy().to_string()
            });
            
            // Get absolute path
            let abs_guild_path = fs::canonicalize(&guild_path)
                .unwrap_or_else(|_| PathBuf::from(&guild_path));
            let abs_path_str = abs_guild_path.to_string_lossy();

            println!("🕯️ Setting up Tellar service with guild: {}", abs_path_str);

            if !systemd_user_dir.exists() {
                fs::create_dir_all(&systemd_user_dir).expect("Failed to create systemd user directory");
            }

            // Embed the template
            let template = include_str!("../../scripts/tellar.service");
            let service_content = template.replace("{{GUILD_PATH}}", &abs_path_str);

            fs::write(&target_service_path, service_content).expect("Failed to write service file");
            println!("✅ Service file created at {:?}", target_service_path);

            // Reload systemd
            run_cmd("systemctl", &["--user", "daemon-reload"]);
            run_cmd("systemctl", &["--user", "enable", service_name]);

            // Enable linger
            let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
            println!("🔧 Enabling linger for {}...", user);
            run_cmd("loginctl", &["enable-linger", &user]);

            println!("🚀 Setup complete. Run 'tellarctl start' to begin stewardship.");
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
