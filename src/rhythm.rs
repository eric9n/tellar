/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/rhythm.rs
 * Responsibility: The Rhythm. The ghost that pulses the Workspace, breathing life into persistent Threads.
 */

use chrono::{Local};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;
use once_cell::sync::Lazy;

/// Metadata format for autonomous threads
#[derive(Deserialize, Debug)]
pub struct ThreadMetadata {
    pub discord_event_id: Option<String>,   // Anchor to Discord Event
    pub schedule: Option<String>,         // Cron expression
    pub injection_template: Option<String>, // What to append
    #[allow(dead_code)]
    pub origin_channel: Option<String>,     // Bound channel
}

type JobMap = Arc<RwLock<HashMap<PathBuf, Uuid>>>;

static SCHEDULER: Lazy<Arc<RwLock<Option<JobScheduler>>>> = Lazy::new(|| Arc::new(RwLock::new(None)));
static JOB_MAP: Lazy<JobMap> = Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

pub async fn run_rhythm(base_path: &Path) -> anyhow::Result<()> {
    let sched = JobScheduler::new().await?;
    {
        let mut lock = SCHEDULER.write().await;
        *lock = Some(sched.clone());
    }

    let rituals_dir = base_path.join("rituals");
    if !rituals_dir.exists() { return Ok(()); }

    // 1. Initial Scan
    let mut initial_threads = Vec::new();
    collect_thread_files(&rituals_dir, &mut initial_threads)?;

    for path in initial_threads {
        let _ = sync_job_from_file(&path).await;
    }

    // 2. Start scheduler
    sched.start().await?;
    println!("💓 The Rhythm is pulsing...");
    Ok(())
}

/// Reactive: Sync a job from a specific file
pub async fn sync_job_from_file(path: &PathBuf) -> anyhow::Result<()> {
    let sched_lock = SCHEDULER.read().await;
    let sched = match &*sched_lock {
        Some(s) => s,
        None => return Ok(()), // Not initialized yet
    };

    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if file_name == "KNOWLEDGE.md" || regex::Regex::new(r"^\d{4}-\d{2}-\d{2}\.md$").unwrap().is_match(file_name) {
        return Ok(());
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    if let Some((header, _)) = parse_thread_metadata(&content) {
        // Only allow scheduling for files linked to a Discord Event (Rituals)
        if header.discord_event_id.is_none() {
            handle_file_removal(path).await?;
            return Ok(());
        }

        if let (Some(cron_expr), Some(template)) = (header.schedule, header.injection_template) {
            if cron_expr.is_empty() { 
                handle_file_removal(path).await?;
                return Ok(()); 
            }

            // Remove existing job
            handle_file_removal(path).await?;

            println!("👻 Ghosting: [{}] with rhythm [{}]", file_name, cron_expr);

            let path_clone = path.clone();
            let template_clone = template.to_string();

            let job = Job::new_async(cron_expr.as_str(), move |_uuid, _l| {
                let path_exec = path_clone.clone();
                let injection = template_clone.clone();
                
                Box::pin(async move {
                    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                    
                    if let Ok(mut current_content) = fs::read_to_string(&path_exec) {
                        let block = format!(
                            "\n\n--- [Ghostly Injection: {}] ---\n{}",
                            timestamp, injection
                        );
                        current_content.push_str(&block);
                        
                        let updated = current_content.replace("status: waiting_for_human", "status: active");

                        if let Err(e) = fs::write(&path_exec, updated) {
                            eprintln!("❌ Ghost failed to inscribe thread {:?}: {:?}", path_exec, e);
                        } else {
                            println!("✍️ Ghost inscribed thread: {:?}", path_exec.file_name().unwrap());
                        }
                    }
                })
            })?;

            let job_id = sched.add(job).await?;
            let mut map = JOB_MAP.write().await;
            map.insert(path.clone(), job_id);
        } else {
            handle_file_removal(path).await?;
        }
    }
    Ok(())
}

/// Reactive: Handle file removal by stopping the job
pub async fn handle_file_removal(path: &PathBuf) -> anyhow::Result<()> {
    let mut map = JOB_MAP.write().await;
    if let Some(job_id) = map.remove(path) {
        let sched_lock = SCHEDULER.read().await;
        if let Some(sched) = &*sched_lock {
            let _ = sched.remove(&job_id).await;
            println!("🗑️ Rhythm removed for: {:?}", path.file_name().unwrap_or_default());
        }
    }
    Ok(())
}

fn parse_thread_metadata(content: &str) -> Option<(ThreadMetadata, &str)> {
    if !content.starts_with("---") { return None; }
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 { return None; }
    let yaml_str = parts[1];
    let body = parts[2].trim();
    if let Ok(header) = serde_yaml::from_str::<ThreadMetadata>(yaml_str) {
        Some((header, body))
    } else {
        None
    }
}

fn collect_thread_files(dir: &Path, paths: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !dir.is_dir() { return Ok(()); }
    let dir_name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if dir_name == "history" || dir_name == "brain" { return Ok(()); }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

        if path.is_dir() {
            collect_thread_files(&path, paths)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
            if file_name == "KNOWLEDGE.md" { continue; }
            let re_stream = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}\.md$").unwrap();
            if re_stream.is_match(file_name) { continue; }
            paths.push(path);
        }
    }
    Ok(())
}