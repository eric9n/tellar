/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/discord/ingest_store.rs
 * Responsibility: Persist inbound Discord messages and attachments into the local guild workspace.
 */

use std::fs;
use std::path::{Component, Path, PathBuf};

pub fn append_to_message_log(
    workspace_path: &Path,
    thread_id: &str,
    author_name: &str,
    author_id: &str,
    content_text: &str,
    message_id: &str,
    timestamp: &str,
    reply_to: Option<String>,
    attachments: Vec<(String, Option<PathBuf>)>,
) -> anyhow::Result<()> {
    let file_path = resolve_thread_log_path(workspace_path, thread_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid thread target: {}", thread_id))?;

    if !file_path.exists()
        && let Some(parent) = file_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

    let mut content = std::fs::read_to_string(&file_path).unwrap_or_default();

    let mut entry = format!(
        "\n---\n**Author**: {} (ID: {}) | **Time**: {} | **Message ID**: {}\n",
        author_name, author_id, timestamp, message_id
    );

    if let Some(reply_id) = reply_to {
        entry.push_str(&format!("**Reply To**: {}\n", reply_id));
    }

    if !attachments.is_empty() {
        entry.push_str("**Attachments**: ");
        let links: Vec<String> = attachments
            .iter()
            .map(|(url, local)| {
                if let Some(lp) = local {
                    let rel = lp
                        .strip_prefix(workspace_path)
                        .unwrap_or(lp)
                        .to_str()
                        .unwrap_or("");
                    format!(
                        "[{}]({}) (local: [file://{}])",
                        lp.file_name().and_then(|s| s.to_str()).unwrap_or("file"),
                        url,
                        rel
                    )
                } else {
                    format!("[link]({})", url)
                }
            })
            .collect();
        entry.push_str(&links.join(", "));
        entry.push('\n');
    }

    entry.push_str(&format!("\n{}\n", content_text));

    content.push_str(&entry);
    std::fs::write(&file_path, content)?;
    Ok(())
}

pub async fn download_attachment(
    workspace_path: &Path,
    attachment: &serenity::model::channel::Attachment,
    message_id: &str,
) -> anyhow::Result<PathBuf> {
    let attachments_dir = workspace_path.join("brain").join("attachments");
    if !attachments_dir.exists() {
        fs::create_dir_all(&attachments_dir)?;
    }

    let filename = format!(
        "{}_{}",
        message_id,
        sanitize_local_filename(&attachment.filename)
    );
    let target_path = attachments_dir.join(filename);

    if target_path.exists() {
        return Ok(target_path);
    }

    let client = reqwest::Client::new();
    let response = client.get(&attachment.url).send().await?;
    let bytes = response.bytes().await?;
    std::fs::write(&target_path, bytes)?;

    Ok(target_path)
}

pub fn resolve_thread_log_path(workspace_path: &Path, thread_id: &str) -> Option<PathBuf> {
    let thread_path = Path::new(thread_id);
    if thread_path.is_absolute() {
        return None;
    }

    if thread_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }

    let channels_dir = workspace_path.join("channels");
    let mut file_path = channels_dir.join(thread_path);
    if !file_path.exists() && !thread_id.ends_with(".md") {
        file_path = channels_dir.join(format!("{}.md", thread_id));
    }

    Some(file_path)
}

pub fn sanitize_local_filename(name: &str) -> String {
    let basename = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment.bin");
    let cleaned: String = basename
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if cleaned.is_empty() {
        "attachment.bin".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_thread_log_path, sanitize_local_filename};
    use tempfile::tempdir;

    #[test]
    fn test_resolve_thread_log_path_rejects_parent_escape() {
        let dir = tempdir().unwrap();

        let resolved = resolve_thread_log_path(dir.path(), "../brain/notes");

        assert!(resolved.is_none());
    }

    #[test]
    fn test_resolve_thread_log_path_accepts_nested_channel_relative_path() {
        let dir = tempdir().unwrap();

        let resolved = resolve_thread_log_path(dir.path(), "general-123456/2026-03-04.md");

        assert_eq!(
            resolved,
            Some(
                dir.path()
                    .join("channels")
                    .join("general-123456")
                    .join("2026-03-04.md")
            )
        );
    }

    #[test]
    fn test_sanitize_local_filename_strips_path_components() {
        assert_eq!(
            sanitize_local_filename("../nested/evil name?.txt"),
            "evil_name_.txt"
        );
    }
}
