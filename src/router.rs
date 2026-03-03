/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/router.rs
 * Responsibility: Lightweight conversational request routing before the agent loop.
 */

use crate::skills::find_explicit_tool_match;
use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RequestRoute {
    DirectTool { tool_name: String, args: Value },
    UnsupportedRealtime { reason: String },
    PlainConversation,
}

fn extract_symbol(text: &str) -> Option<String> {
    let full = Regex::new(r"\b([A-Z]{1,8}\.(?:US|HK|CN))\b").unwrap();
    if let Some(caps) = full.captures(text) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }

    let bare = Regex::new(r"\b([A-Z]{1,6})\b").unwrap();
    bare.captures(text)
        .and_then(|caps| caps.get(1))
        .map(|m| format!("{}.US", m.as_str()))
}

fn extract_expiry(text: &str) -> Option<String> {
    let expiry = Regex::new(r"\b(20\d{2}-\d{2}-\d{2})\b").unwrap();
    expiry
        .captures(text)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

pub(crate) fn extract_trigger_message(full_context: &str, trigger_id: Option<&str>) -> String {
    if let Some(id) = trigger_id {
        let marker = format!("**Message ID**: {}", id);
        if let Some(marker_pos) = full_context.find(&marker) {
            let after_marker = &full_context[marker_pos + marker.len()..];
            if let Some(body_start_rel) = after_marker.find("\n\n") {
                let body_start = marker_pos + marker.len() + body_start_rel + 2;
                let after_body = &full_context[body_start..];
                let body_end = after_body
                    .find("\n---\n**Author**:")
                    .map(|offset| body_start + offset)
                    .unwrap_or(full_context.len());
                let extracted = full_context[body_start..body_end].trim();
                if !extracted.is_empty() {
                    return extracted.to_string();
                }
            }
        }
    }

    let anchor = "> [Tellar]";
    if let Some(pos) = full_context.rfind(anchor) {
        let increment = &full_context[pos..];
        if let Some(msg_start) = increment.find("\n---\n**Author**") {
            return increment[msg_start..].trim().to_string();
        }
        return "Check for follow-up or ritual steps.".to_string();
    }

    full_context.to_string()
}

fn looks_like_realtime_external_query(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    text.contains("天气")
        || lowered.contains("weather")
        || lowered.contains("汇率")
        || lowered.contains("exchange rate")
        || lowered.contains("新闻")
        || lowered.contains("news")
}

fn looks_like_plain_conversation(text: &str) -> bool {
    let trimmed = text.trim();
    let lowered = trimmed.to_ascii_lowercase();

    if trimmed.is_empty() || trimmed.len() > 48 {
        return false;
    }

    if extract_symbol(trimmed).is_some() || extract_expiry(trimmed).is_some() {
        return false;
    }

    let taskish_markers = [
        "用 ",
        "使用",
        "查看",
        "看下",
        "抓取",
        "读取",
        "执行",
        "生成",
        "分析",
        "report",
        "stock_quote",
        "option_",
        "market-tone",
        "snapshot",
        "portfolio",
        "/",
    ];

    if taskish_markers
        .iter()
        .any(|marker| trimmed.contains(marker) || lowered.contains(marker))
    {
        return false;
    }

    let conversational_markers = [
        "这么直接",
        "哈哈",
        "好的",
        "谢谢",
        "在吗",
        "你是谁",
        "什么意思",
        "怎么回事",
        "why",
        "really",
        "thanks",
        "hello",
        "hi",
    ];

    conversational_markers
        .iter()
        .any(|marker| trimmed.contains(marker) || lowered.contains(marker))
}

pub(crate) fn classify_conversational_request(base_path: &Path, text: &str) -> Option<RequestRoute> {
    if let Some(tool_name) = find_explicit_tool_match(base_path, text) {
        let args = match tool_name.as_str() {
            "stock_quote" | "option_expiries" | "probe" => {
                extract_symbol(text).map(|symbol| json!({ "symbol": symbol }))
            }
            "option_quote" | "analyze_option" => {
                extract_symbol(text).map(|symbol| json!({ "symbol": symbol }))
            }
            "option_chain" | "analyze_chain" | "market_tone" | "skew" | "smile" | "put_call_bias" => {
                match (extract_symbol(text), extract_expiry(text)) {
                    (Some(symbol), Some(expiry)) => Some(json!({ "symbol": symbol, "expiry": expiry })),
                    _ => None,
                }
            }
            "market_extreme" | "iv_rank" | "signal_history" => {
                extract_symbol(text).map(|symbol| json!({ "symbol": symbol }))
            }
            "relative_extreme" => {
                let symbol = extract_symbol(text)?;
                Some(json!({ "symbol": symbol, "benchmark": "QQQ.US" }))
            }
            _ => None,
        };

        if let Some(args) = args {
            return Some(RequestRoute::DirectTool { tool_name, args });
        }
    }

    let lowered = text.to_ascii_lowercase();
    if (text.contains("股价") || lowered.contains("stock price") || lowered.contains("quote"))
        && !lowered.contains("option")
        && let Some(symbol) = extract_symbol(text)
    {
        return Some(RequestRoute::DirectTool {
            tool_name: "stock_quote".to_string(),
            args: json!({ "symbol": symbol }),
        });
    }

    if (text.contains("到期日") || lowered.contains("expir"))
        && let Some(symbol) = extract_symbol(text)
    {
        return Some(RequestRoute::DirectTool {
            tool_name: "option_expiries".to_string(),
            args: json!({ "symbol": symbol }),
        });
    }

    if looks_like_realtime_external_query(text) {
        return Some(RequestRoute::UnsupportedRealtime {
            reason: "This looks like a real-time external information request, but no matching live data skill is installed for that category. I should say that directly instead of searching local files.".to_string(),
        });
    }

    if looks_like_plain_conversation(text) {
        return Some(RequestRoute::PlainConversation);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_skill(base: &Path, dir_name: &str, body: &str) {
        let skill_dir = base.join("skills").join(dir_name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn test_classify_conversational_request_routes_explicit_tool() {
        let dir = tempdir().unwrap();
        write_skill(
            dir.path(),
            "snapshot",
            r#"---
name: snapshot
tools:
  stock_quote:
    description: Quote
    shell: ./snapshot.sh
    parameters:
      type: object
---
snapshot guidance
"#,
        );

        let dispatch = classify_conversational_request(
            dir.path(),
            "用 snapshot 的 stock_quote 看一下 TSLA.US 的实时股价",
        )
        .unwrap();

        match dispatch {
            RequestRoute::DirectTool { tool_name, args } => {
                assert_eq!(tool_name, "stock_quote");
                assert_eq!(args["symbol"], "TSLA.US");
            }
            _ => panic!("expected direct tool route"),
        }
    }

    #[test]
    fn test_classify_conversational_request_rejects_weather_query_without_tool() {
        let dir = tempdir().unwrap();
        let dispatch = classify_conversational_request(dir.path(), "益阳天气如何？").unwrap();

        match dispatch {
            RequestRoute::UnsupportedRealtime { .. } => {}
            _ => panic!("expected unsupported realtime route"),
        }
    }

    #[test]
    fn test_classify_conversational_request_routes_short_small_talk() {
        let dir = tempdir().unwrap();
        let dispatch = classify_conversational_request(dir.path(), "这么直接的吗");

        match dispatch {
            Some(RequestRoute::PlainConversation) => {}
            _ => panic!("expected plain conversation route"),
        }
    }

    #[test]
    fn test_extract_trigger_message_prefers_exact_message_id_block() {
        let content = concat!(
            "\n---\n**Author**: Dagow (ID: 1) | **Time**: t1 | **Message ID**: old\n\n",
            "用 snapshot 的 stock_quote 看一下 TSLA.US 的实时股价\n",
            "\n---\n**Author**: Dagow (ID: 1) | **Time**: t2 | **Message ID**: new\n\n",
            "益阳天气如何？ <@1475406915889533049>\n",
        );

        let extracted = extract_trigger_message(content, Some("new"));
        assert_eq!(extracted, "益阳天气如何？ <@1475406915889533049>");
    }
}
