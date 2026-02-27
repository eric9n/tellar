use clap::{Parser, Subcommand};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use anyhow::{Context, Result};
use dirs::home_dir;
use include_dir::{include_dir, Dir};
use tellar::config::Config;

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets");

#[derive(Parser)]
#[command(name = "tellarctl")]
#[command(about = "Tellar CLI: The Intelligent Installer & Manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// 盟友会馆 (Guild) 目录 (默认: ~/.tellar/guild)
    #[arg(short, long, global = true)]
    guild: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Comprehensive environment-aware setup (Keys + Assets + Service)
    Setup,
    /// Start the Tellar steward
    Start,
    /// Stop the Tellar steward
    Stop,
    /// Restart the Tellar steward
    Restart,
    /// Check the steward status
    Status,
    /// View real-time logs
    Logs,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let guild_path = cli.guild.unwrap_or_else(tellar::default_guild_path);
    
    match cli.command {
        Commands::Setup => run_setup(&guild_path).await?,
        Commands::Start => run_service_cmd("start")?,
        Commands::Stop => run_service_cmd("stop")?,
        Commands::Restart => run_service_cmd("restart")?,
        Commands::Status => run_service_cmd("status")?,
        Commands::Logs => run_logs()?,
    }
    Ok(())
}

async fn run_setup(guild_path: &Path) -> Result<()> {
    println!(r#"
    ✨ Tellar Setup - Initializing your Cyber Steward...
    "#);

    // 1. Initialize Foundations
    println!("📂 Target Guild: {}", guild_path.display());
    if !guild_path.exists() {
        fs::create_dir_all(guild_path).context("Failed to create guild directory")?;
    }
    
    // Extract guild structure from embedded assets
    if let Some(guild_dir) = ASSETS.get_dir("guild") {
        println!("📦 Extracting workspace assets...");
        extract_dir_contents(guild_dir, guild_path).context("Failed to extract guild contents")?;
    }

    // 2. Interactive Configuration
    let config_file = guild_path.join("tellar.yml");
    let mut config = Config::load(&config_file).unwrap_or_else(|_| {
         serde_yaml::from_str("gemini:\n  api_key: \"YOUR_KEY\"\n  model: \"\"\ndiscord:\n  token: \"YOUR_TOKEN\"").unwrap()
    });

    // Gemini Keys
    if config.gemini.api_key.contains("YOUR_") || config.gemini.api_key.is_empty() {
        println!("\n🔑 Enter your Gemini API Key:");
        print!("> ");
        std::io::stdout().flush()?;
        let mut key = String::new();
        std::io::stdin().read_line(&mut key)?;
        let trimmed = key.trim();
        if !trimmed.is_empty() { config.gemini.api_key = trimmed.to_string(); }
    }

    // Model Fetching
    if config.gemini.api_key != "YOUR_KEY" {
        println!("\n🤖 Selecting Cyber Brain (Gemini Model)...");
        if let Ok(models) = tellar::llm::list_models(&config.gemini.api_key).await {
            for (i, m) in models.iter().enumerate() {
                println!("  {}. {}", i + 1, m);
            }
            print!("Select (default 1): ");
            std::io::stdout().flush()?;
            let mut choice = String::new();
            std::io::stdin().read_line(&mut choice)?;
            let idx = choice.trim().parse::<usize>().unwrap_or(1).saturating_sub(1);
            config.gemini.model = models.get(idx).unwrap_or(&models[0]).clone();
        }
    }

    // Discord Token
    if config.discord.token.contains("YOUR_") || config.discord.token.is_empty() {
        println!("\n💬 Enter your Discord Bot Token:");
        print!("> ");
        std::io::stdout().flush()?;
        let mut token = String::new();
        std::io::stdin().read_line(&mut token)?;
        let trimmed = token.trim();
        if !trimmed.is_empty() { config.discord.token = trimmed.to_string(); }
    }

    // Save config
    let updated_yaml = serde_yaml::to_string(&config)?;
    fs::write(&config_file, updated_yaml)?;
    println!("\n📝 Configuration inscribed to tellar.yml!");

    // 3. Environment Specific: Systemd (Linux only)
    if cfg!(target_os = "linux") && is_systemctl_available() {
        println!("\n⚙️  Linux detected. Preparing systemd service...");
        if let Some(service_file) = ASSETS.get_file("tellar.service") {
            let template = service_file.contents_utf8().context("Invalid service template")?;
            let current_exe = std::env::current_exe().ok();
            let binary_path = current_exe.and_then(|p| p.parent().map(|d| d.join("tellar")))
                .unwrap_or_else(|| PathBuf::from("tellar"));
            let abs_guild = fs::canonicalize(guild_path).unwrap_or_else(|_| guild_path.to_path_buf());
            
            let updated = template
                .replace("{{GUILD_PATH}}", &abs_guild.to_string_lossy())
                .replace("{{BINARY_PATH}}", &binary_path.to_string_lossy());
            
            let home = home_dir().context("No home directory")?;
            let systemd_dir = home.join(".config").join("systemd").join("user");
            fs::create_dir_all(&systemd_dir)?;
            
            let target_path = systemd_dir.join("tellar.service");
            fs::write(&target_path, updated)?;
            println!("✅ Service file installed at {:?}", target_path);

            println!("🔧 Enabling service...");
            run_cmd("systemctl", &["--user", "daemon-reload"]);
            run_cmd("systemctl", &["--user", "enable", "tellar"]);
            
            let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
            run_cmd("loginctl", &["enable-linger", &user]);
        }
    } else if cfg!(target_os = "macos") {
        println!("\n🍏 macOS detected. Local setup complete.");
        println!("Note: Auto-start service is not yet implemented for macOS (Launchd).");
    }

    println!("\n🚀 Setup complete! Your Steward is ready.");
    Ok(())
}

fn is_systemctl_available() -> bool {
    Command::new("systemctl").arg("--version").output().is_ok()
}

fn run_service_cmd(action: &str) -> Result<()> {
    if !is_systemctl_available() {
        anyhow::bail!("systemctl is not available on this system.");
    }
    run_cmd("systemctl", &["--user", action, "tellar"]);
    Ok(())
}

fn run_logs() -> Result<()> {
     if !is_systemctl_available() {
        anyhow::bail!("journalctl is not available on this system.");
    }
    run_cmd("journalctl", &["--user", "-u", "tellar", "-f"]);
    Ok(())
}

fn run_cmd(cmd: &str, args: &[&str]) {
    let _ = Command::new(cmd).args(args).status();
}

fn extract_dir_contents(dir: &Dir, target: &Path) -> Result<()> {
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(d) => {
                let name = Path::new(d.path()).file_name().context("Invalid dir name in assets")?;
                let new_target = target.join(name);
                fs::create_dir_all(&new_target)?;
                extract_dir_contents(d, &new_target)?;
            }
            include_dir::DirEntry::File(f) => {
                let name = Path::new(f.path()).file_name().context("Invalid file name in assets")?;
                let target_file = target.join(name);
                fs::write(target_file, f.contents())?;
            }
        }
    }
    Ok(())
}
