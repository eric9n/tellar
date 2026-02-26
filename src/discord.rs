/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/discord.rs
 * Responsibility: Discord Inscriber. Perception layer powered by Serenity.
 */

use serenity::async_trait;
use serenity::model::channel::{Message, GuildChannel};
use serenity::model::gateway::{Ready, GatewayIntents};
use serenity::model::guild::ScheduledEvent;
use serenity::prelude::*;

use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use chrono::{Local, Datelike, Timelike};
use crate::StewardNotification;
use tokio::sync::mpsc;


struct Inscriber {
    workspace_path: PathBuf,
    mappings: Arc<RwLock<HashMap<String, String>>>,
    notif_tx: mpsc::Sender<StewardNotification>,

}

#[async_trait]
impl EventHandler for Inscriber {
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot { return; }

        let channel_id_str = msg.channel_id.to_string();
        let folder_name = {
            let mut found = None;
            {
                let map = self.mappings.read().await;
                if let Some(target) = map.get(&channel_id_str) {
                    found = Some(target.clone());
                }
            }

            if let Some(f) = found {
                f
            } else {
                // Dynamic Discovery: Try to resolve physically first, then via Discord
                let mut resolved = self.resolve_physical_folder(&channel_id_str).unwrap_or_else(|| channel_id_str.clone());
                
                if resolved == channel_id_str {
                    if let Ok(channel) = ctx.http.get_channel(msg.channel_id.into()).await {
                        if let Some(guild_ch) = channel.guild() {
                            resolved = to_folder_name(&guild_ch.name, &channel_id_str);
                        }
                    }
                }

                
                println!("🔍 Dynamically mapped channel: #{} -> {}", channel_id_str, resolved);
                
                {
                    let mut map = self.mappings.write().await;
                    map.insert(channel_id_str.clone(), resolved.clone());

                }

                let folder_path = self.workspace_path.join("channels").join(&resolved);
                if !folder_path.exists() {
                    let _ = fs::create_dir_all(&folder_path);
                }
                resolved
            }
        };

        let is_mention = msg.mentions_user_id(ctx.cache.current_user().id) || msg.content.starts_with("!do");
        
        let author_name = msg.author.name.clone();
        let author_id = msg.author.id.to_string();
        let message_id = msg.id.to_string();
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let reply_to = msg.referenced_message.as_ref().map(|m| m.id.to_string());
        let content = msg.content.clone();

        // 1. Download Attachments
        let mut attachment_data = Vec::new();
        for attachment in &msg.attachments {
            match self.download_attachment(attachment, &message_id).await {
                Ok(local_path) => {
                    attachment_data.push((attachment.url.clone(), Some(local_path)));
                }
                Err(e) => {
                    eprintln!("⚠️ Failed to download attachment {}: {:?}", attachment.filename, e);
                    attachment_data.push((attachment.url.clone(), None));
                }
            }
        }

        if is_mention {
            println!("📥 Discord mention captured for #{}: {}", folder_name, content);
            
            let today = Local::now().format("%Y-%m-%d").to_string();
            let daily_file = format!("{}.md", today);
            let target_path = self.workspace_path.join("channels").join(&folder_name).join(&daily_file);
            
            let _ = self.append_to_message_log(&format!("{}/{}", folder_name, daily_file), 
                &author_name, &author_id, &content, &message_id, &timestamp, reply_to.clone(), attachment_data.clone());

            let _ = self.notif_tx.send(StewardNotification {
                blackboard_path: target_path,
                channel_id: channel_id_str,
                guild_id: msg.guild_id.map(|id| id.to_string()).unwrap_or_else(|| "0".to_string()),
                message_id: message_id.clone(),
                content: content.clone(),
            }).await;


        } else if let Some(referenced) = &msg.referenced_message {
            if referenced.author.bot && referenced.content.contains("[Thread: ") {
                if let Some(pos) = referenced.content.find("[Thread: ") {
                    let start = pos + 9;
                    if let Some(end) = referenced.content[start..].find(']') {
                        let thread_id = &referenced.content[start..start+end];
                        println!("💬 Captured reply to thread: {} for id: {}", content, thread_id);
                        
                        let _ = self.append_to_message_log(thread_id, 
                            &author_name, &author_id, &content, &message_id, &timestamp, reply_to, attachment_data);
                    }
                }
            }
        } else {
            let today = Local::now().format("%Y-%m-%d").to_string();
            let daily_file = format!("{}.md", today);
            let target = format!("{}/{}", folder_name, daily_file);
            
            let _ = self.append_to_message_log(&target, 
                &author_name, &author_id, &content, &message_id, &timestamp, reply_to, attachment_data);
        }
    }

    async fn channel_create(&self, _ctx: Context, channel: GuildChannel) {
        let channel_id = channel.id.to_string();
        
        // 1. Try to find existing folder by ID suffix first (Self-Healing)
        let folder_name = self.resolve_physical_folder(&channel_id)
            .unwrap_or_else(|| to_folder_name(&channel.name, &channel_id));
        
        println!("✨ New channel detected: #{} ({})", channel.name, folder_name);
        
        {
            let mut map = self.mappings.write().await;
            map.insert(channel_id, folder_name.clone());
        }

        let folder_path = self.workspace_path.join("channels").join(&folder_name);
        if !folder_path.exists() {
            let _ = fs::create_dir_all(&folder_path);
        }
    }

    async fn channel_update(&self, _ctx: Context, _old: Option<GuildChannel>, new: GuildChannel) {
        let channel_id = new.id.to_string();
        let new_folder_name = to_folder_name(&new.name, &channel_id);
        
        // Find existing folder by ID suffix (Robust Anchor)
        let current_folder = self.resolve_physical_folder(&channel_id);

        if let Some(old) = current_folder {
            if old != new_folder_name {
                println!("📝 Channel renamed: #{} -> #{}", old, new_folder_name);
                
                let old_path = self.workspace_path.join("channels").join(&old);
                let new_path = self.workspace_path.join("channels").join(&new_folder_name);
                
                if old_path.exists() {
                    if let Err(e) = fs::rename(&old_path, &new_path) {
                        eprintln!("⚠️ Failed to rename local folder from {} to {}: {:?}", old, new_folder_name, e);
                    } else {
                        println!("📂 Local folder synchronized: {} -> {}", old, new_folder_name);
                    }
                }
                
                // Update mapping cache
                let mut map = self.mappings.write().await;
                map.insert(channel_id, new_folder_name);
            }
        } else {
            // Case where folders were deleted or never existed
            let mut map = self.mappings.write().await;
            map.insert(channel_id, new_folder_name);
        }
    }

    async fn guild_scheduled_event_create(&self, _ctx: Context, event: ScheduledEvent) {
        println!("📅 Discord Event created: {}", event.name);
        self.sync_event_to_brain(&event);
    }

    async fn guild_scheduled_event_update(&self, _ctx: Context, event: ScheduledEvent) {
        println!("📅 Discord Event updated: {}", event.name);
        self.sync_event_to_brain(&event);
    }

    async fn guild_scheduled_event_delete(&self, _ctx: Context, event: ScheduledEvent) {
        println!("🗑️ Discord Event deleted: {}", event.name);
        let brain_event_path = self.workspace_path.join("brain").join("events").join(format!("event_{}.json", event.id));
        let _ = fs::remove_file(brain_event_path);
    }

    async fn message_delete(&self, _ctx: Context, _channel_id: serenity::model::id::ChannelId, deleted_message_id: serenity::model::id::MessageId, _guild_id: Option<serenity::model::id::GuildId>) {
        let msg_id_str = deleted_message_id.to_string();
        println!("🗑️ Discord Message deleted: {}", msg_id_str);
        let _ = self.scrub_message_from_logs(&msg_id_str);
    }

    async fn message_delete_bulk(&self, _ctx: Context, _channel_id: serenity::model::id::ChannelId, multiple_deleted_message_ids: Vec<serenity::model::id::MessageId>, _guild_id: Option<serenity::model::id::GuildId>) {
        println!("🗑️ Discord Bulk Message deletion: {} messages", multiple_deleted_message_ids.len());
        for msg_id in multiple_deleted_message_ids {
            let _ = self.scrub_message_from_logs(&msg_id.to_string());
        }
    }

    async fn ready(&self, _: Context, ready: Ready) {
        println!("✅ {} is connected and inscribing!", ready.user.name);
    }
}

impl Inscriber {
    fn append_to_message_log(
        &self, 
        thread_id: &str, 
        author_name: &str, 
        author_id: &str,
        content_text: &str,
        message_id: &str,
        timestamp: &str,
        reply_to: Option<String>,
        attachments: Vec<(String, Option<PathBuf>)>
    ) -> anyhow::Result<()> {
        let mut file_path = self.workspace_path.join("channels").join(thread_id);
        if !file_path.exists() && !thread_id.ends_with(".md") {
            file_path = self.workspace_path.join("channels").join(format!("{}.md", thread_id));
        }

        if !file_path.exists() {
            if let Some(parent) = file_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
        }

        let mut content = fs::read_to_string(&file_path).unwrap_or_default();


        
        let mut entry = format!(
            "\n---\n**Author**: {} (ID: {}) | **Time**: {} | **Message ID**: {}\n",
            author_name, author_id, timestamp, message_id
        );

        if let Some(reply_id) = reply_to {
            entry.push_str(&format!("**Reply To**: {}\n", reply_id));
        }

        if !attachments.is_empty() {
            entry.push_str("**Attachments**: ");
            let links: Vec<String> = attachments.iter().map(|(url, local)| {
                if let Some(lp) = local {
                    let rel = lp.strip_prefix(&self.workspace_path).unwrap_or(lp).to_str().unwrap_or("");
                    format!("[{}]({}) (local: [file://{}])", lp.file_name().and_then(|s| s.to_str()).unwrap_or("file"), url, rel)
                } else {
                    format!("[link]({})", url)
                }
            }).collect();
            entry.push_str(&links.join(", "));
            entry.push_str("\n");
        }

        entry.push_str(&format!("\n{}\n", content_text));
        
        content.push_str(&entry);
        
        fs::write(&file_path, content)?;
        Ok(())
    }

    async fn download_attachment(&self, attachment: &serenity::model::channel::Attachment, message_id: &str) -> anyhow::Result<PathBuf> {
        let attachments_dir = self.workspace_path.join("brain").join("attachments");
        if !attachments_dir.exists() {
            fs::create_dir_all(&attachments_dir)?;
        }

        let filename = format!("{}_{}", message_id, attachment.filename);
        let target_path = attachments_dir.join(filename);

        if target_path.exists() {
            return Ok(target_path);
        }

        let client = reqwest::Client::new();
        let response = client.get(&attachment.url).send().await?;
        let bytes = response.bytes().await?;
        fs::write(&target_path, bytes)?;

        Ok(target_path)
    }

    fn sync_event_to_brain(&self, event: &ScheduledEvent) {
        let brain_dir = self.workspace_path.join("brain").join("events");
        if !brain_dir.exists() {
            let _ = fs::create_dir_all(&brain_dir);
        }
        let brain_event_path = brain_dir.join(format!("event_{}.json", event.id));
        if let Ok(json_data) = serde_json::to_string(event) {
            let _ = fs::write(brain_event_path, json_data);
        }
    }

    fn scrub_message_from_logs(&self, message_id: &str) -> anyhow::Result<()> {
        let channels_dir = self.workspace_path.join("channels");
        if !channels_dir.exists() { return Ok(()); }

        let pattern = format!("**Message ID**: {}", message_id);
        
        for entry in fs::read_dir(channels_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                for file_entry in fs::read_dir(path)? {
                    let file_entry = file_entry?;
                    let file_path = file_entry.path();
                    if file_path.extension().and_then(|s| s.to_str()) == Some("md") {
                        if let Ok(content) = fs::read_to_string(&file_path) {
                            if content.contains(&pattern) {
                                let new_content = self.remove_message_block(&content, &pattern);
                                if new_content != content {
                                    fs::write(&file_path, new_content)?;
                                    println!("✂️ Scrubbed message {} from {:?}", message_id, file_path.file_name().unwrap());
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn remove_message_block(&self, content: &str, pattern: &str) -> String {
        let lines: Vec<&str> = content.lines().collect();
        let mut target_index = None;
        
        for (i, line) in lines.iter().enumerate() {
            if line.contains(pattern) {
                target_index = Some(i);
                break;
            }
        }

        if let Some(idx) = target_index {
            let mut start_idx = idx;
            while start_idx > 0 {
                if lines[start_idx] == "---" {
                    break;
                }
                start_idx -= 1;
            }

            let mut end_idx = idx + 1;
            while end_idx < lines.len() {
                if lines[end_idx] == "---" {
                    break;
                }
                end_idx += 1;
            }

            let mut new_lines = lines;
            new_lines.drain(start_idx..end_idx);
            return new_lines.join("\n");
        }
        
        content.to_string()
    }

    /// Robust Folder Resolution: Find a folder by its ID suffix anchor
    fn resolve_physical_folder(&self, channel_id: &str) -> Option<String> {
        resolve_folder_by_id(&self.workspace_path, channel_id)
    }
}

pub fn resolve_folder_by_id(workspace_path: &Path, channel_id: &str) -> Option<String> {
    let suffix = if channel_id.len() >= 6 {
        &channel_id[channel_id.len() - 6..]
    } else {
        channel_id
    };
    let anchor = format!("({})", suffix);
    
    let channels_dir = workspace_path.join("channels");
    if let Ok(entries) = fs::read_dir(channels_dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(&anchor) {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}

pub async fn start_listening(
    token: &str, 
    workspace_path: PathBuf, 
    mappings: Arc<RwLock<HashMap<String, String>>>,
    notif_tx: mpsc::Sender<StewardNotification>,
) -> anyhow::Result<()> {
    let handler = Inscriber {
        workspace_path,
        mappings,
        notif_tx,
    };

    let intents = GatewayIntents::GUILD_MESSAGES 
        | GatewayIntents::DIRECT_MESSAGES 
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILDS
        | GatewayIntents::GUILD_SCHEDULED_EVENTS;

    let mut client = Client::builder(token, intents)
        .event_handler(handler)
        .await?;

    client.start().await?;
    Ok(())
}

use once_cell::sync::Lazy;

static HTTP_CHANNEL_CLIENT: Lazy<Arc<RwLock<Option<(String, Arc<serenity::http::Http>)>>>> = Lazy::new(|| Arc::new(RwLock::new(None)));

/// Helper to get or create the shared Http client
async fn get_http_client(token: &str) -> Arc<serenity::http::Http> {

    let mut client_lock = HTTP_CHANNEL_CLIENT.write().await;
    let reuse = if let Some((existing_token, _)) = &*client_lock {
        existing_token == token
    } else {
        false
    };

    if !reuse {
        let new_http = Arc::new(serenity::http::Http::new(token));
        *client_lock = Some((token.to_string(), new_http.clone()));
        new_http
    } else {
        client_lock.as_ref().unwrap().1.clone()
    }
}

/// Send a message directly using the Bot Token
pub async fn send_bot_message(token: &str, channel_id: &str, content: &str) -> anyhow::Result<serenity::model::channel::Message> {
    if token.is_empty() { return Err(anyhow::anyhow!("Discord token is empty")); }
    if channel_id.is_empty() || channel_id == "0" { 
        return Err(anyhow::anyhow!("Invalid channel ID: {}", channel_id)); 
    }
    
    println!("📡 Sending Discord message to channel {} ({} chars)...", channel_id, content.len());
    
    let http = get_http_client(token).await;

    let c_id = channel_id.parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid channel ID: {}", channel_id))?;
    
    let map = serde_json::json!({ "content": content });
    let msg = http.send_message(c_id.into(), vec![], &map).await?;
    Ok(msg)
}


/// Trigger the typing indicator in a channel
pub async fn broadcast_typing(token: &str, channel_id: &str) -> anyhow::Result<()> {
    if token.is_empty() || channel_id.is_empty() || channel_id == "0" { return Ok(()); }
    
    let http = get_http_client(token).await;
    let c_id = channel_id.parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid channel ID: {}", channel_id))?;
    
    http.broadcast_typing(c_id.into()).await?;
    Ok(())
}





/// Helper to extract raw Discord Channel ID from a folder name (e.g. "General (123456)")
pub fn extract_id_from_folder(folder_name: &str) -> Option<String> {
    let re = regex::Regex::new(r"\((\d+)\)$").ok()?;
    re.captures(folder_name).and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
}

/// Helper to fetch channels on startup (adapter for serenity)
pub async fn fetch_guild_channels(token: &str, guild_id: &str) -> anyhow::Result<HashMap<String, String>> {
    let http = serenity::http::Http::new(token);
    let g_id = guild_id.parse::<u64>()?;
    let channels = http.get_channels(g_id.into()).await?;
    
    let mut map = HashMap::new();
    for channel in channels {
        if channel.kind == serenity::model::channel::ChannelType::Text {
            let folder_name = to_folder_name(&channel.name, &channel.id.to_string());
            map.insert(channel.id.to_string(), folder_name);
        }
    }
    Ok(map)
}

/// Synchronize a Discord Scheduled Event to a local thread
pub async fn sync_discord_event(
    base_path: &Path, 
    event_id: &str, 
    name: &str, 
    channel_id: Option<&str>, 
    start_time: &str, 
    status: i64
) -> anyhow::Result<()> {
    let target_dir = base_path.join("rituals");
    if !target_dir.exists() {
        fs::create_dir_all(&target_dir)?;
    }

    let mut thread_path = None;
    if let Ok(entries) = fs::read_dir(&target_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if content.contains(&format!("discord_event_id: \"{}\"", event_id)) {
                        thread_path = Some(path);
                        break;
                    }
                }
            }
        }
    }

    let cron_expr = match chrono::DateTime::parse_from_rfc3339(start_time) {
        Ok(dt) => {
            let dt = dt.with_timezone(&chrono::Utc);
            format!("0 {} {} {} {} *", dt.minute(), dt.hour(), dt.day(), dt.month())
        },
        Err(_) => "".to_string(),
    };

    let thread_status = if status == 1 { "active" } else { "pending_approval" };
    let origin = channel_id.unwrap_or("0"); // Use raw Discord ID or 0

    let content = format!(
r#"-----
discord_event_id: "{}"
task_id: "ritual_{}"
origin_channel: "{}"
status: {}
schedule: "{}"
injection_template: |
  - [ ] Start the Ritual: {}
-----
# Ritual: {}

This ritual is synchronized with a Discord Scheduled Event.
Event ID: {}
Start Time (UTC): {}
"#, event_id, event_id, origin, thread_status, cron_expr, name, name, event_id, start_time);

    let final_path = thread_path.unwrap_or_else(|| {
        let safe_name = name.to_lowercase().replace(' ', "_").replace(|c: char| !c.is_alphanumeric() && c != '_', "");
        target_dir.join(format!("ritual_{}_{}.md", safe_name, event_id))
    });

    fs::write(final_path, content)?;
    println!("🌌 Ritual synchronized: {} (ID: {})", name, event_id);

    Ok(())
}

/// Reconcile all Discord events from the brain
pub async fn sync_all_discord_events(base_path: &Path, _mappings: Option<Arc<RwLock<HashMap<String, String>>>>) -> anyhow::Result<()> {
    let brain_dir = base_path.join("brain").join("events");
    if !brain_dir.exists() { return Ok(()); }

    for entry in fs::read_dir(brain_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                    let event_id = data["id"].as_str().unwrap_or("");
                    let name = data["name"].as_str().unwrap_or("");
                    let start_time = data["scheduled_start_time"].as_str().unwrap_or("");
                    let status = data["status"].as_i64().unwrap_or(0);
                    let channel_id = data["channel_id"].as_str();

                    let _ = sync_discord_event(base_path, event_id, name, channel_id, start_time, status).await;
                }
            }
        }
    }
    Ok(())
}

/// Helper to generate collision-resistant folder names
pub fn to_folder_name(name: &str, id: &str) -> String {
    let suffix = &id[id.len().saturating_sub(6)..];
    format!("{} ({})", name, suffix)
}