/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/lib.rs
 * Responsibility: Shared library modules
 */

pub mod config;
pub mod init;
pub mod llm;
pub mod steward;
pub mod rhythm;
pub mod discord;
pub mod guardian;
pub mod skills;
pub mod watch;

use std::path::PathBuf;

#[derive(Debug)]
pub struct StewardNotification {
    pub blackboard_path: PathBuf,
    pub channel_id: String,
    pub guild_id: String,
    pub message_id: String,
    pub content: String,
}
