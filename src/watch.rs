/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/watch.rs
 * Responsibility: The Watchman. Global filesystem observer that awakens roles based on blackboard inscriptions.
 */

use crate::StewardNotification;
use crate::config::Config;
use crate::thread_runtime;
use notify::{
    EventKind, RecursiveMode, Watcher,
    event::{CreateKind, ModifyKind},
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::mpsc;

#[derive(Debug, PartialEq, Eq)]
enum WatchAction {
    SyncBrainEvents,
    ExecuteRitual,
    Ignore,
}

fn is_relevant_fs_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Create(CreateKind::Any)
            | EventKind::Create(CreateKind::File)
    )
}

fn classify_watch_path(path: &Path, brain_dir: &Path, rituals_dir: &Path) -> WatchAction {
    if path.starts_with(brain_dir) && path.extension().and_then(|s| s.to_str()) == Some("json") {
        WatchAction::SyncBrainEvents
    } else if path.starts_with(rituals_dir)
        && path.extension().and_then(|s| s.to_str()) == Some("md")
    {
        WatchAction::ExecuteRitual
    } else {
        WatchAction::Ignore
    }
}

pub async fn start_watchman(
    base_path: &Path,
    config: &Config,
    mut notif_rx: mpsc::Receiver<StewardNotification>,
    mappings: Arc<RwLock<HashMap<String, String>>>,
) -> anyhow::Result<()> {
    let brain_dir = base_path.join("brain");
    let channels_dir = base_path.join("channels");
    let rituals_dir = base_path.join("rituals");

    for dir in &[&brain_dir, &channels_dir, &rituals_dir] {
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
        }
    }

    println!("👁️ The Watchman is observing brain/, channels/, and rituals/...");

    let (fs_tx, mut fs_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                if fs_tx.send(event).is_err() {
                    eprintln!(
                        "⚠️ Watchman dropped a filesystem event because the receiver is closed."
                    );
                }
            }
            Err(error) => {
                eprintln!("⚠️ Watchman filesystem watcher error: {:?}", error);
            }
        })?;

    watcher.watch(&brain_dir, RecursiveMode::Recursive)?;
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
                let _ = thread_runtime::execute_thread_file(
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
                if is_relevant_fs_event(&event.kind) {
                    for path in event.paths {
                        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

                        match classify_watch_path(&path, &brain_dir, &rituals_dir) {
                            WatchAction::SyncBrainEvents => {
                                let _ = crate::discord::sync_all_discord_events(&base_path_clone, Some(mappings.clone())).await;
                            }
                            WatchAction::ExecuteRitual => {
                                println!("⚙️ Watchman detected ritual edit: {:?}, awakening Steward...", file_name);
                                let _ = thread_runtime::execute_thread_file(&path, &base_path_clone, &config_clone, None, None, None).await;
                            }
                            WatchAction::Ignore => {
                                // Channels are intentionally passive to filesystem events.
                                // They only react to Discord message signals (MPSC).
                            }
                        }
                    }
                }
            },

            else => break,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{DataChange, ModifyKind};
    use std::path::Path;

    #[test]
    fn test_is_relevant_fs_event_filters_noise() {
        assert!(is_relevant_fs_event(&EventKind::Create(CreateKind::File)));
        assert!(is_relevant_fs_event(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(!is_relevant_fs_event(&EventKind::Access(
            notify::event::AccessKind::Any
        )));
    }

    #[test]
    fn test_classify_watch_path_routes_expected_targets() {
        let brain_dir = Path::new("/tmp/guild/brain");
        let rituals_dir = Path::new("/tmp/guild/rituals");

        assert_eq!(
            classify_watch_path(
                Path::new("/tmp/guild/brain/events/evt.json"),
                brain_dir,
                rituals_dir
            ),
            WatchAction::SyncBrainEvents
        );
        assert_eq!(
            classify_watch_path(
                Path::new("/tmp/guild/rituals/daily.md"),
                brain_dir,
                rituals_dir
            ),
            WatchAction::ExecuteRitual
        );
        assert_eq!(
            classify_watch_path(
                Path::new("/tmp/guild/channels/general/2026-02-27.md"),
                brain_dir,
                rituals_dir
            ),
            WatchAction::Ignore
        );
    }
}
