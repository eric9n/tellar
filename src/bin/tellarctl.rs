use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use dirs::home_dir;
use include_dir::{include_dir, Dir};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tellar::config::{Config, DiscordConfig, GeminiConfig, RuntimeConfig};

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets");

#[derive(Parser)]
#[command(name = "tellarctl")]
#[command(about = "Tellar CLI: installer and service manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Guild workspace path (default: ~/.tellar/guild)
    #[arg(short, long, global = true)]
    guild: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize config and workspace assets
    Setup {
        /// Overwrite existing embedded asset files
        #[arg(long)]
        force: bool,
    },
    /// Install or update the Linux systemd user service
    InstallService,
    /// Start the Tellar user service
    Start,
    /// Stop the Tellar user service
    Stop,
    /// Restart the Tellar user service
    Restart,
    /// Show the Tellar user service status
    Status,
    /// Tail Tellar service logs
    Logs,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let guild_path = cli.guild.unwrap_or_else(tellar::default_guild_path);

    match cli.command {
        Commands::Setup { force } => run_setup(&guild_path, force).await?,
        Commands::InstallService => install_linux_service(&guild_path)?,
        Commands::Start => run_service_cmd("start")?,
        Commands::Stop => run_service_cmd("stop")?,
        Commands::Restart => run_service_cmd("restart")?,
        Commands::Status => run_service_cmd("status")?,
        Commands::Logs => run_logs()?,
    }

    Ok(())
}

async fn run_setup(guild_path: &Path, force: bool) -> Result<()> {
    println!("Tellar setup");
    println!("Target guild: {}", guild_path.display());

    fs::create_dir_all(guild_path).context("failed to create guild directory")?;

    if let Some(guild_dir) = ASSETS.get_dir("guild") {
        println!("Installing workspace assets...");
        let stats = extract_dir_contents(guild_dir, guild_path, force)?;
        println!(
            "Assets installed: {} created, {} overwritten, {} preserved",
            stats.created_files, stats.overwritten_files, stats.skipped_files
        );
    }

    let config_file = guild_path.join("tellar.yml");
    let mut config = load_or_default_config(&config_file)?;

    if needs_value(&config.gemini.api_key) {
        config.gemini.api_key = prompt_required("Enter your Gemini API key")?;
    }

    if !needs_value(&config.gemini.api_key) {
        configure_model(&mut config).await?;
    }

    if needs_value(&config.discord.token) {
        config.discord.token = prompt_required("Enter your Discord bot token")?;
    }

    save_config(&config_file, &config)?;
    println!("Configuration written to {}", config_file.display());

    if cfg!(target_os = "linux") {
        println!("Linux detected. Run `tellarctl install-service` to install the systemd user service.");
    } else if cfg!(target_os = "macos") {
        println!("macOS detected. Workspace setup is complete.");
        println!("Service installation is not implemented for Launchd yet.");
    } else {
        println!("Workspace setup is complete.");
        println!("Service management is currently only implemented for Linux systemd.");
    }

    println!("Setup complete.");
    Ok(())
}

fn load_or_default_config(path: &Path) -> Result<Config> {
    match Config::load(path) {
        Ok(config) => Ok(config),
        Err(_) => Ok(Config {
            gemini: GeminiConfig {
                api_key: "YOUR_KEY".to_string(),
                model: String::new(),
            },
            discord: DiscordConfig {
                token: "YOUR_TOKEN".to_string(),
                guild_id: None,
                channel_mappings: None,
            },
            runtime: RuntimeConfig::default(),
            guardian: None,
        }),
    }
}

fn save_config(path: &Path, config: &Config) -> Result<()> {
    let yaml = serde_yaml::to_string(config).context("failed to serialize config")?;
    fs::write(path, yaml).with_context(|| format!("failed to write {}", path.display()))
}

fn needs_value(value: &str) -> bool {
    value.trim().is_empty() || value.contains("YOUR_")
}

fn prompt_required(label: &str) -> Result<String> {
    loop {
        print!("{}: ", label);
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read stdin")?;
        let trimmed = input.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }

        println!("A value is required.");
    }
}

async fn configure_model(config: &mut Config) -> Result<()> {
    if !config.gemini.model.trim().is_empty() && !config.gemini.model.contains("YOUR_") {
        println!("Using configured Gemini model: {}", config.gemini.model);
        return Ok(());
    }

    println!("Fetching available Gemini models...");
    let models = tellar::llm::list_models(&config.gemini.api_key)
        .await
        .context("failed to fetch Gemini models")?;

    if models.is_empty() {
        bail!("Gemini returned no selectable models");
    }

    for (index, model) in models.iter().enumerate() {
        println!("  {}. {}", index + 1, model);
    }

    print!("Select a model (default 1): ");
    io::stdout().flush().context("failed to flush stdout")?;
    let mut choice = String::new();
    io::stdin()
        .read_line(&mut choice)
        .context("failed to read stdin")?;
    let index = choice.trim().parse::<usize>().unwrap_or(1).saturating_sub(1);
    config.gemini.model = models
        .get(index)
        .cloned()
        .unwrap_or_else(|| models[0].clone());

    Ok(())
}

fn install_linux_service(guild_path: &Path) -> Result<()> {
    require_command("systemctl")?;
    require_command("loginctl")?;

    println!("Linux detected. Installing systemd user service...");
    let template = ASSETS
        .get_file("tellar.service")
        .context("missing embedded tellar.service asset")?
        .contents_utf8()
        .context("invalid service template encoding")?;

    let tellar_binary = find_tellar_binary()?;
    let abs_guild = fs::canonicalize(guild_path).unwrap_or_else(|_| guild_path.to_path_buf());

    let rendered = render_service_template(template, &abs_guild, &tellar_binary);

    let home = home_dir().context("could not determine home directory")?;
    let systemd_dir = home.join(".config").join("systemd").join("user");
    fs::create_dir_all(&systemd_dir).context("failed to create systemd user directory")?;

    let service_path = systemd_dir.join("tellar.service");
    fs::write(&service_path, rendered)
        .with_context(|| format!("failed to write {}", service_path.display()))?;
    println!("Installed service file at {}", service_path.display());

    run_checked_cmd("systemctl", &["--user", "daemon-reload"])?;
    run_checked_cmd("systemctl", &["--user", "enable", "tellar"])?;

    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    run_checked_cmd("loginctl", &["enable-linger", &user])?;

    println!("Systemd user service is enabled. Start it with `tellarctl start`.");
    Ok(())
}

fn find_tellar_binary() -> Result<PathBuf> {
    let current_exe = std::env::current_exe().context("failed to resolve tellarctl path")?;
    resolve_tellar_binary_from_current_exe(&current_exe)
}

fn resolve_tellar_binary_from_current_exe(current_exe: &Path) -> Result<PathBuf> {
    let sibling = current_exe
        .parent()
        .map(|dir| dir.join("tellar"))
        .ok_or_else(|| anyhow!("failed to resolve tellar binary directory"))?;

    if sibling.is_file() {
        Ok(sibling)
    } else {
        bail!(
            "could not find the `tellar` binary next to `tellarctl` at {}",
            sibling.display()
        );
    }
}

fn render_service_template(template: &str, guild_path: &Path, binary_path: &Path) -> String {
    template
        .replace("{{GUILD_PATH}}", &guild_path.to_string_lossy())
        .replace("{{BINARY_PATH}}", &binary_path.to_string_lossy())
}

fn run_service_cmd(action: &str) -> Result<()> {
    ensure_systemd_service_support()?;
    run_checked_cmd("systemctl", &["--user", action, "tellar"])
}

fn run_logs() -> Result<()> {
    ensure_linux()?;
    require_command("journalctl")?;
    run_checked_cmd("journalctl", &["--user", "-u", "tellar", "-f"])
}

fn ensure_systemd_service_support() -> Result<()> {
    ensure_linux()?;
    require_command("systemctl")
}

fn ensure_linux() -> Result<()> {
    if cfg!(target_os = "linux") {
        Ok(())
    } else if cfg!(target_os = "macos") {
        bail!("service commands are not implemented for macOS yet");
    } else {
        bail!("service commands are only implemented for Linux systemd");
    }
}

fn require_command(cmd: &str) -> Result<()> {
    let status = Command::new(cmd)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to execute `{}`", cmd))?;

    if status.success() {
        Ok(())
    } else {
        bail!("`{}` is not available on this system", cmd);
    }
}

fn run_checked_cmd(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("failed to execute `{}`", cmd))?;

    if status.success() {
        Ok(())
    } else {
        bail!(
            "`{}` exited with status {}",
            format_command(cmd, args),
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string())
        );
    }
}

fn format_command(cmd: &str, args: &[&str]) -> String {
    if args.is_empty() {
        cmd.to_string()
    } else {
        format!("{} {}", cmd, args.join(" "))
    }
}

#[derive(Default)]
struct ExtractStats {
    created_files: usize,
    overwritten_files: usize,
    skipped_files: usize,
}

fn extract_dir_contents(dir: &Dir, target: &Path, force: bool) -> Result<ExtractStats> {
    let mut stats = ExtractStats::default();

    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(child_dir) => {
                let name = Path::new(child_dir.path())
                    .file_name()
                    .context("invalid asset directory name")?;
                let new_target = target.join(name);
                fs::create_dir_all(&new_target)
                    .with_context(|| format!("failed to create {}", new_target.display()))?;
                let child_stats = extract_dir_contents(child_dir, &new_target, force)?;
                stats.created_files += child_stats.created_files;
                stats.overwritten_files += child_stats.overwritten_files;
                stats.skipped_files += child_stats.skipped_files;
            }
            include_dir::DirEntry::File(file) => {
                let name = Path::new(file.path())
                    .file_name()
                    .context("invalid asset file name")?;
                let target_file = target.join(name);
                if target_file.exists() {
                    if !force {
                        stats.skipped_files += 1;
                        continue;
                    }
                    stats.overwritten_files += 1;
                } else {
                    stats.created_files += 1;
                }

                fs::write(&target_file, file.contents())
                    .with_context(|| format!("failed to write {}", target_file.display()))?;
            }
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_format_command_renders_args() {
        assert_eq!(format_command("systemctl", &["--user", "status", "tellar"]), "systemctl --user status tellar");
        assert_eq!(format_command("journalctl", &[]), "journalctl");
    }

    #[test]
    fn test_load_or_default_config_uses_placeholders_for_missing_file() {
        let dir = tempdir().unwrap();
        let config = load_or_default_config(&dir.path().join("missing.yml")).unwrap();

        assert_eq!(config.gemini.api_key, "YOUR_KEY");
        assert_eq!(config.discord.token, "YOUR_TOKEN");
        assert!(config.gemini.model.is_empty());
    }

    #[test]
    fn test_render_service_template_replaces_placeholders() {
        let rendered = render_service_template(
            "ExecStart={{BINARY_PATH}} --guild {{GUILD_PATH}}\nWorkingDirectory={{GUILD_PATH}}",
            Path::new("/tmp/guild"),
            Path::new("/usr/local/bin/tellar"),
        );

        assert!(rendered.contains("ExecStart=/usr/local/bin/tellar --guild /tmp/guild"));
        assert!(rendered.contains("WorkingDirectory=/tmp/guild"));
        assert!(!rendered.contains("{{GUILD_PATH}}"));
        assert!(!rendered.contains("{{BINARY_PATH}}"));
    }

    #[test]
    fn test_resolve_tellar_binary_from_current_exe_uses_sibling_binary() {
        let dir = tempdir().unwrap();
        let current_exe = dir.path().join("tellarctl");
        let tellar = dir.path().join("tellar");
        fs::write(&current_exe, "").unwrap();
        fs::write(&tellar, "").unwrap();

        let resolved = resolve_tellar_binary_from_current_exe(&current_exe).unwrap();
        assert_eq!(resolved, tellar);
    }

    #[test]
    fn test_resolve_tellar_binary_from_current_exe_errors_when_missing() {
        let dir = tempdir().unwrap();
        let current_exe = dir.path().join("tellarctl");
        fs::write(&current_exe, "").unwrap();

        let err = resolve_tellar_binary_from_current_exe(&current_exe).unwrap_err();
        assert!(format!("{}", err).contains("could not find the `tellar` binary"));
    }

    #[test]
    fn test_extract_dir_contents_preserves_existing_files() {
        let dir = tempdir().unwrap();
        let guild_dir = ASSETS.get_dir("guild").unwrap();

        let first = extract_dir_contents(guild_dir, dir.path(), false).unwrap();
        assert!(first.created_files > 0);

        let existing_path = dir.path().join("agents").join("AGENTS.md");
        let original = fs::read_to_string(&existing_path).unwrap();
        fs::write(&existing_path, "customized").unwrap();

        let second = extract_dir_contents(guild_dir, dir.path(), false).unwrap();
        assert_eq!(second.created_files, 0);
        assert_eq!(second.overwritten_files, 0);
        assert!(second.skipped_files > 0);
        assert_eq!(fs::read_to_string(&existing_path).unwrap(), "customized");

        fs::write(&existing_path, original).unwrap();
    }

    #[test]
    fn test_extract_dir_contents_force_overwrites_existing_files() {
        let dir = tempdir().unwrap();
        let guild_dir = ASSETS.get_dir("guild").unwrap();

        extract_dir_contents(guild_dir, dir.path(), false).unwrap();

        let existing_path = dir.path().join("agents").join("AGENTS.md");
        let original = fs::read_to_string(&existing_path).unwrap();
        fs::write(&existing_path, "customized").unwrap();

        let stats = extract_dir_contents(guild_dir, dir.path(), true).unwrap();
        assert_eq!(stats.created_files, 0);
        assert!(stats.overwritten_files > 0);
        assert_eq!(fs::read_to_string(&existing_path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn test_run_checked_cmd_returns_error_on_non_zero_status() {
        let err = run_checked_cmd("sh", &["-c", "exit 7"]).unwrap_err();
        let message = format!("{}", err);
        assert!(message.contains("exited with status 7"));
    }
}
