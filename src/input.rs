/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/input.rs
 * Responsibility: Normalize wake signals and raw conversation logs into worksets.
 */

use once_cell::sync::Lazy;
use regex::Regex;

static CONVERSATION_HEADER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\*\*Author\*\*: (.*?) \| \*\*Time\*\*:.*?(?: \| \*\*Message ID\*\*: ([^\n]+))?$")
        .expect("valid conversation header regex")
});
static MENTION_ONLY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(?:<@!?\d+>\s*)+$").expect("valid mention regex"));

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Workset {
    messages: Vec<String>,
}

impl Workset {
    pub(crate) fn new(messages: Vec<String>) -> Self {
        Self { messages }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub(crate) fn text(&self) -> String {
        self.messages.join("\n\n")
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ConversationEntry {
    author: String,
    message_id: Option<String>,
    body: String,
}

fn parse_conversation_entries(full_context: &str) -> Vec<ConversationEntry> {
    let normalized = format!("\n{}", full_context);

    normalized
        .split("\n---\n")
        .filter_map(|chunk| {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                return None;
            }

            let (header, body) = chunk.split_once("\n\n")?;
            let caps = CONVERSATION_HEADER_RE.captures(header.trim())?;
            let author = caps.get(1)?.as_str().trim().to_string();
            let message_id = caps.get(2).map(|m| m.as_str().trim().to_string());
            Some(ConversationEntry {
                author,
                message_id,
                body: body.trim().to_string(),
            })
        })
        .collect()
}

fn is_wake_only_message(body: &str) -> bool {
    MENTION_ONLY_RE.is_match(body.trim())
}

fn collect_fallback_workset(full_context: &str) -> Workset {
    let text = full_context.trim();
    if text.is_empty() {
        Workset::new(Vec::new())
    } else {
        Workset::new(vec![text.to_string()])
    }
}

fn resolve_trigger_index(entries: &[ConversationEntry], trigger_id: Option<&str>) -> usize {
    trigger_id
        .and_then(|id| {
            entries
                .iter()
                .rposition(|entry| entry.message_id.as_deref() == Some(id))
        })
        .unwrap_or_else(|| entries.len().saturating_sub(1))
}

fn find_pending_window_start(entries: &[ConversationEntry], trigger_index: usize) -> usize {
    entries[..trigger_index]
        .iter()
        .rposition(|entry| entry.author.contains("Tellar"))
        .map(|index| index + 1)
        .unwrap_or(0)
}

fn collect_pending_messages(
    entries: &[ConversationEntry],
    start_index: usize,
    trigger_index: usize,
) -> Vec<String> {
    entries[start_index..=trigger_index]
        .iter()
        .filter(|entry| !entry.author.contains("Tellar"))
        .filter(|entry| !entry.body.is_empty())
        .filter(|entry| !is_wake_only_message(&entry.body))
        .map(|entry| entry.body.clone())
        .collect()
}

pub(crate) fn collect_pending_workset(full_context: &str, trigger_id: Option<&str>) -> Workset {
    let entries = parse_conversation_entries(full_context);
    if entries.is_empty() {
        return collect_fallback_workset(full_context);
    }

    let trigger_index = resolve_trigger_index(&entries, trigger_id);
    let start_index = find_pending_window_start(&entries, trigger_index);
    let pending_messages = collect_pending_messages(&entries, start_index, trigger_index);

    Workset::new(pending_messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_pending_workset_uses_messages_since_last_tellar_reply() {
        let content = concat!(
            "---\n**Author**: Dagow (ID: 1) | **Time**: t1 | **Message ID**: old\n\n",
            "用 snapshot 的 stock_quote 看一下 TSLA.US 的实时股价\n",
            "\n---\n**Author**: Tellar (ID: 2) | **Time**: t2 | **Message ID**: bot\n\n",
            "{json}\n",
            "\n---\n**Author**: Dagow (ID: 1) | **Time**: t3 | **Message ID**: ask\n\n",
            "益阳天气如何？\n",
            "\n---\n**Author**: Dagow (ID: 1) | **Time**: t4 | **Message ID**: ping\n\n",
            "<@1475406915889533049>\n",
        );

        let extracted = collect_pending_workset(content, Some("ping"));
        assert_eq!(extracted.text(), "益阳天气如何？");
    }

    #[test]
    fn test_collect_pending_workset_preserves_single_message_mode() {
        let content = concat!(
            "---\n**Author**: Dagow (ID: 1) | **Time**: t1 | **Message ID**: only\n\n",
            "看下 TSLA 的股价\n",
        );

        let extracted = collect_pending_workset(content, Some("only"));
        assert_eq!(extracted.text(), "看下 TSLA 的股价");
    }
}
