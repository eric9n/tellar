/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/watch.rs
 * Responsibility: The Watchman. Global filesystem observer that awakens roles based on blackboard inscriptions.
 */

use notify::{Watcher, RecursiveMode, EventKind, event::{ModifyKind, CreateKind}};
use std::path::Path;
use crate::config::Config;
use crate::steward;
use crate::StewardNotification;
use tokio::sync::mpsc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub async fn start_watchman(
    base_path: &Path, 
    config: &Config, 
    mut notif_rx: mpsc::Receiver<StewardNotification>,
    mappings: Arc<RwLock<HashMap<String, String>>>
) -> anyhow::Result<()> {
    let channels_dir = base_path.join("channels");
    let rituals_dir = base_path.join("rituals");
    
    for dir in &[&channels_dir, &rituals_dir] {
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
        }
    }

    println!("👁️ The Watchman is observing the Blackboards (channels/ & rituals/)...");

    let (fs_tx, mut fs_rx) = tokio::sync::mpsc::channel(100);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let _ = fs_tx.blocking_send(event);
        }
    })?;

    watcher.watch(&channels_dir, RecursiveMode::Recursive)?;
    watcher.watch(&rituals_dir, RecursiveMode::Recursive)?;

    let base_path_clone = base_path.to_path_buf();
    let config_clone = config.clone();

    loop {
        tokio::select! {
            // Priority 1: Conversational Notifications (MPSC Trigger)
            Some(notif) = notif_rx.recv() => {
                println!("📢 Watchman received signal: awakens Steward...");
                // Trigger immediate execution with full context
                let _ = steward::execute_thread_file(
                    &notif.blackboard_path, 
                    &base_path_clone, 
                    &config_clone, 
                    Some(notif.message_id), 
                    Some(notif.channel_id),
                    Some(notif.guild_id)
                ).await;


            },
            
            // Priority 2: Filesystem Events (Watch Trigger - System/Non-Conversational)
            Some(event) = fs_rx.recv() => {
                match event.kind {
                    EventKind::Modify(ModifyKind::Data(_)) | EventKind::Modify(ModifyKind::Any) | EventKind::Create(CreateKind::Any) | EventKind::Create(CreateKind::File) => {
                        for path in event.paths {
                            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                            
                            if path.to_str().unwrap().contains("brain") && path.extension().and_then(|s| s.to_str()) == Some("json") {
                                // Discord Event sync (Global Brain)
                                let _ = crate::discord::sync_all_discord_events(&base_path_clone, Some(mappings.clone())).await;
                            } else if path.starts_with(&rituals_dir) && path.extension().and_then(|s| s.to_str()) == Some("md") {
                                // 🚀 Awakening: Reactive Ritual Trigger
                                println!("⚙️ Watchman detected ritual edit: {:?}, awakening Steward...", file_name);
                                let _ = steward::execute_thread_file(&path, &base_path_clone, &config_clone, None, None, None).await;


                            }
                            // Channels are intentionally passive to filesystem events. 
                            // They only react to Discord message signals (MPSC).
                        }
                    }
                    _ => {}
                }
            },
            
            else => break,
        }
    }

    Ok(())
}
