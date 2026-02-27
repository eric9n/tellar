/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/thread_runtime.rs
 * Responsibility: Execute thread files, dispatch role sessions, and persist results.
 */

use crate::config::Config;
use crate::discord;
use crate::context::{
    extract_channel_id_from_path, is_conversational_log, parse_task_document,
};
use crate::session::{run_conversational_loop, run_react_loop};
use crate::tools::mask_sensitive_data;
use chrono::Local;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

static EXECUTING_FILES: Lazy<Mutex<HashSet<PathBuf>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static CONCURRENCY_LIMITER: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(5)));

fn append_task_result_log(
    content: &str,
    task_line: &str,
    result: &str,
    timestamp: &str,
) -> (String, bool) {
    if !result.starts_with("Error executing task:") && !result.starts_with("Gemini API Error:") {
        let updated_line = task_line.replace("[ ]", "[x]");
        let log_entry = format!("\n> [{}] Execution result: {}", timestamp, result);
        let mut next = content.replace(task_line, &updated_line);
        next.push_str(&log_entry);
        (next, true)
    } else {
        let log_entry = format!("\n> [{}] ❌ Task failed: {}", timestamp, result);
        let mut next = content.to_string();
        next.push_str(&log_entry);
        (next, false)
    }
}

fn should_archive_thread(content: &str, schedule: Option<&str>) -> bool {
    let schedule_value = schedule.unwrap_or("").trim();
    if !schedule_value.is_empty() {
        return false;
    }

    let re_any_todo = Regex::new(r"- \[ \]").unwrap();
    !re_any_todo.is_match(content)
}

fn history_destination(parent: &Path, file_name: &OsStr, date: &str) -> PathBuf {
    parent.join("history").join(date).join(file_name)
}

pub async fn execute_thread_file(
    path: &PathBuf,
    base_path: &Path,
    config: &Config,
    trigger_id: Option<String>,
    target_channel_id: Option<String>,
    target_guild_id: Option<String>,
) -> anyhow::Result<()> {
    {
        let mut executing = EXECUTING_FILES.lock().unwrap();
        if executing.contains(path) {
            return Ok(());
        }
        executing.insert(path.clone());
    }

    let _permit = CONCURRENCY_LIMITER.acquire().await.unwrap();
    let res = execute_thread_file_internal(
        path,
        base_path,
        config,
        trigger_id,
        target_channel_id,
        target_guild_id,
    )
    .await;

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

    let re_todo = Regex::new(r"- \[ \] (.*)").unwrap();

    if !is_log {
        while let Some(caps) = re_todo.captures(&content) {
            let task_line = caps.get(0).unwrap().as_str();
            let task_desc = caps.get(1).unwrap().as_str();

            println!("⚙️ Executing step in #{}: {}", thread_id, task_desc);

            let result = match run_react_loop(
                task_desc,
                &content,
                path,
                base_path,
                config,
                &channel_id,
            )
            .await
            {
                Ok(res) => res,
                Err(e) => {
                    let err_msg = format!("Error executing task: {}", e);
                    println!("❌ {}", err_msg);
                    err_msg
                }
            };

            let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let (next_content, completed) =
                append_task_result_log(&content, task_line, &result, &timestamp);
            content = next_content;

            if completed {
                fs::write(path, &content)?;

                let sanitized_result = mask_sensitive_data(&result, config);
                if let Err(e) = discord::send_bot_message(
                    &config.discord.token,
                    &channel_id,
                    &format!("⚙️ Step completed in **#{}**\n{}", thread_id, sanitized_result),
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
        let _ = discord::broadcast_typing(&config.discord.token, &channel_id).await;

        match run_conversational_loop(
            &content,
            path,
            base_path,
            config,
            trigger_id,
            &channel_id,
        )
        .await
        {
            Ok(result) => {
                let sanitized_result = mask_sensitive_data(&result, config);
                match discord::send_bot_message(
                    &config.discord.token,
                    &channel_id,
                    &sanitized_result,
                )
                .await
                {
                    Ok(msg) => {
                        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                        let bot_name = &msg.author.name;
                        let bot_id = msg.author.id.to_string();
                        let msg_id = msg.id.to_string();

                        let response_entry = format!(
                            "\n---\n**Author**: {} (ID: {}) | **Time**: {} | **Message ID**: {}\n\n{}\n",
                            bot_name, bot_id, timestamp, msg_id, result
                        );
                        content.push_str(&response_entry);
                        let _ = fs::write(path, &content);
                    }
                    Err(e) => {
                        eprintln!("❌ Failed to send Discord message to {}: {:?}", channel_id, e);
                        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                        let response_entry =
                            format!("\n\n> [Tellar] ({}): {}\n", timestamp, result);
                        content.push_str(&response_entry);
                        let _ = fs::write(path, &content);
                    }
                }
            }
            Err(e) => {
                eprintln!("❌ Steward loop failed in #{}: {:?}", thread_id, e);
                let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
                let error_entry = format!(
                    "\n\n> [Tellar] ({}): ❌ Error processing request: {}",
                    timestamp, e
                );
                content.push_str(&error_entry);
                let _ = fs::write(path, &content);

                let _ = discord::send_bot_message(
                    &config.discord.token,
                    &channel_id,
                    "⚠️ *管家在处理您的请求时遇到了异常，请稍后再试，或检查黑板记录。*",
                )
                .await;
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
                        let _ = discord::send_bot_message(
                            &config.discord.token,
                            &channel_id,
                            &format!(
                                "📦 Thread **#{}** has been archived to history/{}",
                                thread_id, today
                            ),
                        )
                        .await;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_append_task_result_log_marks_completed_task() {
        let content = "---\nstatus: open\n---\n- [ ] Ship release";
        let (updated, completed) = append_task_result_log(
            content,
            "- [ ] Ship release",
            "Release shipped successfully",
            "2026-02-27 12:00:00",
        );

        assert!(completed);
        assert!(updated.contains("- [x] Ship release"));
        assert!(updated.contains("Execution result: Release shipped successfully"));
    }

    #[test]
    fn test_append_task_result_log_keeps_failed_task_open() {
        let content = "---\nstatus: open\n---\n- [ ] Ship release";
        let (updated, completed) = append_task_result_log(
            content,
            "- [ ] Ship release",
            "Error executing task: network failed",
            "2026-02-27 12:00:00",
        );

        assert!(!completed);
        assert!(updated.contains("- [ ] Ship release"));
        assert!(updated.contains("❌ Task failed: Error executing task: network failed"));
    }

    #[test]
    fn test_should_archive_thread_requires_no_schedule_and_no_open_todos() {
        assert!(should_archive_thread("---\nstatus: done\n---\n- [x] Finished", None));
        assert!(!should_archive_thread("---\nstatus: done\n---\n- [ ] Pending", None));
        assert!(!should_archive_thread("---\nstatus: done\n---\n- [x] Finished", Some("0 * * * *")));
    }

    #[test]
    fn test_history_destination_builds_expected_path() {
        let parent = Path::new("/tmp/channel");
        let dest = history_destination(parent, OsStr::new("thread.md"), "2026-02-27");
        assert_eq!(dest, Path::new("/tmp/channel/history/2026-02-27/thread.md"));
    }
}
