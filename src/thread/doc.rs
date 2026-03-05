/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/thread/doc.rs
 * Responsibility: Thread document parsing and routing-related file inspection.
 */

use crate::discord;
use serde::Deserialize;

use std::path::Path;

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub(crate) struct TaskHeader {
    pub(crate) status: String,
    pub(crate) schedule: Option<String>,
    pub(crate) injection_template: Option<String>,
    pub(crate) origin_channel: Option<String>,
}

pub(crate) fn parse_task_document(content: &str) -> Option<(TaskHeader, &str)> {
    if !content.starts_with("---") {
        return None;
    }
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        return None;
    }
    let yaml_str = parts[1];
    let body = parts[2].trim();
    if let Ok(header) = serde_yml::from_str::<TaskHeader>(yaml_str) {
        Some((header, body))
    } else {
        None
    }
}

pub(crate) fn is_conversational_log(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    file_name.len() == 13
        && file_name.chars().nth(4) == Some('-')
        && file_name.chars().nth(7) == Some('-')
        && path.extension().and_then(|s| s.to_str()) == Some("md")
}

pub(crate) fn extract_channel_id_from_path(path: &Path) -> String {
    if let Ok(content) = std::fs::read_to_string(path)
        && let Some((header, _)) = parse_task_document(&content)
            && let Some(origin) = header.origin_channel
                && origin != "0" {
                    return origin;
                }

    if let Some(parent) = path.parent()
        && let Some(folder_name) = parent.file_name().and_then(|s| s.to_str())
            && let Some(id) = discord::extract_id_from_folder(folder_name) {
                return id;
            }

    "0".to_string()
}
