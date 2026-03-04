/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/thread/mod.rs
 * Responsibility: Execute thread files, dispatch role sessions, and persist results.
 */

use self::doc::{extract_channel_id_from_path, is_conversational_log, parse_task_document};
use self::store::{
    append_discord_response_log, append_internal_task_error_log, append_local_response_log,
    append_processing_error_log, append_task_result_log, history_destination,
    should_archive_thread,
};
use crate::config::Config;
use crate::discord::client as discord_client;
use crate::session::{execute_ritual_step, run_conversational_loop};
use crate::tools::mask_sensitive_data;
use chrono::Local;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

pub mod doc;
pub mod store;

#[derive(Debug, Clone)]
struct PendingThreadRun {
    trigger_id: Option<String>,
    target_channel_id: Option<String>,
    target_guild_id: Option<String>,
}

static EXECUTING_FILES: Lazy<Mutex<HashSet<PathBuf>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static PENDING_THREAD_RUNS: Lazy<Mutex<HashMap<PathBuf, PendingThreadRun>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static CONCURRENCY_LIMITER: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(5)));
static PENDING_TODO_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"- \[ \] (.*)").expect("valid todo capture regex"));

pub async fn execute_thread_file(
    path: &PathBuf,
    base_path: &Path,
    config: &Config,
    trigger_id: Option<String>,
    target_channel_id: Option<String>,
    target_guild_id: Option<String>,
) -> anyhow::Result<()> {
    let mut next_run = PendingThreadRun {
        trigger_id,
        target_channel_id,
        target_guild_id,
    };

    {
        let mut executing = EXECUTING_FILES.lock().unwrap();
        if executing.contains(path) {
            let mut pending = PENDING_THREAD_RUNS.lock().unwrap();
            pending.insert(path.clone(), next_run);
            return Ok(());
        }
        executing.insert(path.clone());
    }

    let _permit = CONCURRENCY_LIMITER.acquire().await.unwrap();
    let res = loop {
        let PendingThreadRun {
            trigger_id,
            target_channel_id,
            target_guild_id,
        } = next_run;

        let result = execute_thread_file_internal(
            path,
            base_path,
            config,
            trigger_id,
            target_channel_id,
            target_guild_id,
        )
        .await;

        let pending_rerun = {
            let mut pending = PENDING_THREAD_RUNS.lock().unwrap();
            pending.remove(path)
        };

        match pending_rerun {
            Some(pending) => {
                println!(
                    "🔁 Re-running thread {:?} to process a coalesced trigger.",
                    path.file_name()
                );
                next_run = pending;
            }
            None => break result,
        }
    };

    {
        let mut executing = EXECUTING_FILES.lock().unwrap();
        executing.remove(path);
    }

    res
}

async fn execute_thread_file_internal(
    path: &PathBuf,
    base_path: &Path,
    config: &Config,
    trigger_id: Option<String>,
    target_channel_id: Option<String>,
    _target_guild_id: Option<String>,
) -> anyhow::Result<()> {
    let mut content = fs::read_to_string(path)?;

    let is_log = is_conversational_log(path);
    let thread_id = path
        .strip_prefix(base_path.join("channels"))
        .unwrap_or(path)
        .to_str()
        .unwrap_or("unknown");

    let channel_id = match target_channel_id {
        Some(id) => id,
        None => {
            let fallback = extract_channel_id_from_path(path);
            println!(
                "⚠️ Steward using fallback channel ID: {} for {:?}",
                fallback,
                path.file_name()
            );
            fallback
        }
    };

    let header_owned = parse_task_document(&content).map(|(h, _)| h);
    if !is_log && header_owned.is_none() {
        return Ok(());
    }

    if !is_log {
        while let Some(caps) = PENDING_TODO_RE.captures(&content) {
            let task_line = caps.get(0).unwrap().as_str();
            let task_desc = caps.get(1).unwrap().as_str();

            println!("⚙️ Executing step in #{}: {}", thread_id, task_desc);

            let outcome = match execute_ritual_step(
                task_desc,
                &content,
                path,
                base_path,
                config,
                &channel_id,
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(e) => {
                    eprintln!("❌ Error executing task in #{}: {}", thread_id, e);
                    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                    content = append_internal_task_error_log(&content, &timestamp, &e.to_string());
                    fs::write(path, &content)?;
                    break;
                }
            };

            let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let (next_content, completed) =
                append_task_result_log(&content, task_line, &outcome, &timestamp);
            content = next_content;

            if completed {
                fs::write(path, &content)?;

                let sanitized_result = mask_sensitive_data(&outcome.user_response, config);
                if let Err(e) = discord_client::send_bot_message(
                    &config.discord.token,
                    &channel_id,
                    &format!(
                        "⚙️ Step completed in **#{}**\n{}",
                        thread_id, sanitized_result
                    ),
                )
                .await
                {
                    eprintln!(
                        "❌ Failed to send Discord ritual message to {}: {:?}",
                        channel_id, e
                    );
                }
            } else {
                fs::write(path, &content)?;
                break;
            }
        }
    } else {
        println!("🗣️ Conversational Mode in #{}...", thread_id);
        let _ = discord_client::broadcast_typing(&config.discord.token, &channel_id).await;

        match run_conversational_loop(&content, path, base_path, config, trigger_id, &channel_id)
            .await
        {
            Ok(outcome) => {
                println!(
                    "🗣️ Conversational outcome in #{}: {}",
                    thread_id,
                    outcome.log_summary()
                );

                let sanitized_result = mask_sensitive_data(&outcome.user_response, config);
                match discord_client::send_bot_message(
                    &config.discord.token,
                    &channel_id,
                    &sanitized_result,
                )
                .await
                {
                    Ok(msg) => {
                        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                        content = append_discord_response_log(
                            &content,
                            &msg.author.name,
                            &msg.author.id.to_string(),
                            &timestamp.to_string(),
                            &msg.id.to_string(),
                            &outcome.user_response,
                        );
                        if let Err(error) = fs::write(path, &content) {
                            eprintln!(
                                "⚠️ Failed to persist Discord-backed response log for {:?}: {:?}",
                                path.file_name(),
                                error
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "❌ Failed to send Discord message to {}: {:?}",
                            channel_id, e
                        );
                        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                        content = append_local_response_log(
                            &content,
                            &timestamp.to_string(),
                            &outcome.user_response,
                        );
                        if let Err(error) = fs::write(path, &content) {
                            eprintln!(
                                "⚠️ Failed to persist local fallback response log for {:?}: {:?}",
                                path.file_name(),
                                error
                            );
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("❌ Steward loop failed in #{}: {:?}", thread_id, e);
                let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                content =
                    append_processing_error_log(&content, &timestamp.to_string(), &e.to_string());
                if let Err(error) = fs::write(path, &content) {
                    eprintln!(
                        "⚠️ Failed to persist processing error log for {:?}: {:?}",
                        path.file_name(),
                        error
                    );
                }

                if let Err(error) = discord_client::send_bot_message(
                    &config.discord.token,
                    &channel_id,
                    "⚠️ *管家在处理您的请求时遇到了异常，请稍后再试，或检查黑板记录。*",
                )
                .await
                {
                    eprintln!(
                        "⚠️ Failed to send processing-error notification to {}: {:?}",
                        channel_id, error
                    );
                }
            }
        }
    }

    if let Some(header) = header_owned {
        if should_archive_thread(&content, header.schedule.as_deref()) {
            if let Some(parent) = path.parent() {
                let today = Local::now().format("%Y-%m-%d").to_string();
                let history_dir = parent.join("history").join(&today);
                let _ = fs::create_dir_all(&history_dir);

                if let Some(file_name) = path.file_name() {
                    let dest_path = history_destination(parent, file_name, &today);
                    if let Err(e) = fs::rename(path, &dest_path) {
                        eprintln!("⚠️ Failed to archive thread: {:?}", e);
                    } else {
                        println!("📦 Thread archived to history/{}", today);
                        if let Err(error) = discord_client::send_bot_message(
                            &config.discord.token,
                            &channel_id,
                            &format!(
                                "📦 Thread **#{}** has been archived to history/{}",
                                thread_id, today
                            ),
                        )
                        .await
                        {
                            eprintln!(
                                "⚠️ Failed to send archive notification to {}: {:?}",
                                channel_id, error
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
