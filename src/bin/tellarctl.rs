use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use dirs::home_dir;
use include_dir::{include_dir, Dir};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tellar::config::{Config, DiscordConfig, GeminiConfig, RuntimeConfig};

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets");
const SKILL_SCHEMA: &str = include_str!("../../schemas/skill.schema.json");

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
    /// Compile a SKILL.md into a runtime SKILL.json
    InstallSkill {
        /// Path to a skill directory containing SKILL.md
        path: PathBuf,
        /// Overwrite an existing SKILL.json
        #[arg(long)]
        force: bool,
    },
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
        Commands::InstallSkill { path, force } => run_install_skill(&guild_path, &path, force).await?,
        Commands::Start => run_service_cmd("start")?,
        Commands::Stop => run_service_cmd("stop")?,
        Commands::Restart => run_service_cmd("restart")?,
        Commands::Status => run_service_cmd("status")?,
        Commands::Logs => run_logs()?,
    }

    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
struct InstalledSkill {
    name: String,
    description: String,
    #[serde(default)]
    guidance: Option<String>,
    tools: Vec<InstalledSkillTool>,
}

#[derive(Debug, Deserialize, Serialize)]
struct InstalledSkillTool {
    name: String,
    description: String,
    parameters: Value,
    command: String,
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

async fn run_install_skill(guild_path: &Path, skill_path: &Path, force: bool) -> Result<()> {
    let skill_dir = fs::canonicalize(skill_path)
        .with_context(|| format!("failed to resolve skill directory {}", skill_path.display()))?;
    if !skill_dir.is_dir() {
        bail!("skill path is not a directory: {}", skill_dir.display());
    }

    let skill_md = skill_dir.join("SKILL.md");
    if !skill_md.exists() {
        bail!("missing SKILL.md in {}", skill_dir.display());
    }

    let target = skill_dir.join("SKILL.json");
    if target.exists() && !force {
        bail!(
            "{} already exists. Re-run with `--force` to overwrite.",
            target.display()
        );
    }

    let config_path = guild_path.join("tellar.yml");
    let config = Config::load(&config_path).with_context(|| {
        format!(
            "failed to load Tellar config at {} for skill compilation",
            config_path.display()
        )
    })?;
    if needs_value(&config.gemini.api_key) || needs_value(&config.gemini.model) {
        bail!("Gemini API key and model must be configured before installing a skill");
    }

    let skill_md_content = fs::read_to_string(&skill_md)
        .with_context(|| format!("failed to read {}", skill_md.display()))?;
    let tree = collect_skill_tree(&skill_dir)?;
    let prompt = build_skill_install_prompt(&skill_md_content, &tree);

    println!("Compiling skill {} with Gemini...", skill_dir.display());
    let turn = tellar::llm::generate_turn(
        "You compile SKILL.md documents into strict machine-readable SKILL.json files. Output valid JSON only, with no markdown fences and no commentary.",
        vec![tellar::llm::Message {
            role: tellar::llm::MessageRole::User,
            parts: vec![tellar::llm::MultimodalPart::text(prompt)],
        }],
        &config.gemini.api_key,
        &config.gemini.model,
        0.2,
        None,
    )
    .await
    .context("failed to compile skill with Gemini")?;

    let raw_output = match turn {
        tellar::llm::ModelTurn::Narrative(text) => text,
        tellar::llm::ModelTurn::ToolCalls { .. } => {
            bail!("skill compiler unexpectedly returned tool calls")
        }
    };

    let json_payload = extract_json_object(&raw_output)?;
    let compiled: InstalledSkill =
        serde_json::from_str(&json_payload).context("generated SKILL.json is not valid JSON")?;
    validate_installed_skill(&compiled)?;

    let rendered = serde_json::to_string_pretty(&compiled).context("failed to serialize SKILL.json")?;
    fs::write(&target, rendered)
        .with_context(|| format!("failed to write {}", target.display()))?;

    println!(
        "Installed skill `{}` with {} tool(s) -> {}",
        compiled.name,
        compiled.tools.len(),
        target.display()
    );
    for tool in &compiled.tools {
        println!("  - {}", tool.name);
    }

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

fn collect_skill_tree(skill_dir: &Path) -> Result<String> {
    fn walk(base: &Path, current: &Path, out: &mut Vec<String>) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(current)?
            .filter_map(|entry| entry.ok())
            .collect();
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let path = entry.path();
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                out.push(format!("DIR  {}", rel));
                walk(base, &path, out)?;
            } else {
                out.push(format!("FILE {}", rel));
            }
            if out.len() >= 128 {
                break;
            }
        }
        Ok(())
    }

    let mut lines = Vec::new();
    walk(skill_dir, skill_dir, &mut lines)?;
    Ok(lines.join("\n"))
}

fn build_skill_install_prompt(skill_md: &str, tree: &str) -> String {
    format!(
        "Compile the following skill into a strict SKILL.json document.\n\nRequirements:\n- Output JSON only.\n- Conform to this schema exactly.\n- Do not invent files or commands that are not supported by the SKILL.md or directory tree.\n- `tools` must be a non-empty array.\n- Each tool requires `name`, `description`, `parameters`, and `command`.\n- `parameters.type` must be `object`.\n- Use concise but useful descriptions.\n\n### SKILL.json Schema\n{}\n\n### Skill Directory Tree\n{}\n\n### SKILL.md\n{}",
        SKILL_SCHEMA, tree, skill_md
    )
}

fn extract_json_object(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        return Ok(trimmed.to_string());
    }

    let fenced = Regex::new(r"(?s)```(?:json)?\s*(\{.*\})\s*```")
        .expect("valid fenced json regex");
    if let Some(caps) = fenced.captures(trimmed) {
        if let Some(body) = caps.get(1) {
            return Ok(body.as_str().trim().to_string());
        }
    }

    bail!("model output did not contain a JSON object")
}

fn validate_installed_skill(skill: &InstalledSkill) -> Result<()> {
    let name_re = Regex::new(r"^[A-Za-z0-9_-]+$").expect("valid skill name regex");

    if skill.name.trim().is_empty() {
        bail!("skill name cannot be empty");
    }
    if !name_re.is_match(&skill.name) {
        bail!("skill name must match ^[A-Za-z0-9_-]+$");
    }
    if skill.description.trim().is_empty() {
        bail!("skill description cannot be empty");
    }
    if skill.tools.is_empty() {
        bail!("skill must declare at least one tool");
    }

    let mut seen = HashSet::new();
    for tool in &skill.tools {
        if tool.name.trim().is_empty() {
            bail!("tool name cannot be empty");
        }
        if !name_re.is_match(&tool.name) {
            bail!("tool name `{}` must match ^[A-Za-z0-9_-]+$", tool.name);
        }
        if !seen.insert(tool.name.clone()) {
            bail!("duplicate tool name `{}`", tool.name);
        }
        if tool.description.trim().is_empty() {
            bail!("tool `{}` description cannot be empty", tool.name);
        }
        if tool.command.trim().is_empty() {
            bail!("tool `{}` command cannot be empty", tool.name);
        }
        if tool
            .parameters
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            != "object"
        {
            bail!("tool `{}` parameters.type must be `object`", tool.name);
        }
    }

    Ok(())
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

    #[test]
    fn test_extract_json_object_strips_markdown_fence() {
        let json = extract_json_object("```json\n{\"name\":\"demo\"}\n```").unwrap();
        assert_eq!(json, "{\"name\":\"demo\"}");
    }

    #[test]
    fn test_validate_installed_skill_rejects_duplicate_tool_names() {
        let skill = InstalledSkill {
            name: "demo".to_string(),
            description: "desc".to_string(),
            guidance: None,
            tools: vec![
                InstalledSkillTool {
                    name: "dup".to_string(),
                    description: "a".to_string(),
                    parameters: serde_json::json!({ "type": "object" }),
                    command: "printf a".to_string(),
                },
                InstalledSkillTool {
                    name: "dup".to_string(),
                    description: "b".to_string(),
                    parameters: serde_json::json!({ "type": "object" }),
                    command: "printf b".to_string(),
                },
            ],
        };

        let err = validate_installed_skill(&skill).unwrap_err();
        assert!(format!("{}", err).contains("duplicate tool name"));
    }
}
