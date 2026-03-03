/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/router.rs
 * Responsibility: LLM-backed conversational routing before the agent loop.
 */

use crate::config::Config;
use crate::llm;
use crate::tools::get_routing_tool_definitions;
use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PlanStep {
    CallTool { tool_name: String, args: Value },
    Respond { instruction: String },
    AskForMissing { prompt: String },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RequestRoute {
    PlanAndExecute { steps: Vec<PlanStep> },
    Reject { reason: String },
    Agent,
}

#[derive(Debug, Deserialize)]
struct RouteDecision {
    route: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    steps: Vec<RouteDecisionStep>,
}

#[derive(Debug, Deserialize)]
struct RouteDecisionStep {
    kind: String,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    args: Option<Value>,
    #[serde(default)]
    instruction: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct ConversationEntry {
    author: String,
    message_id: Option<String>,
    body: String,
}

fn parse_conversation_entries(full_context: &str) -> Vec<ConversationEntry> {
    let normalized = format!("\n{}", full_context);

    let header_re = Regex::new(
        r"^\*\*Author\*\*: (.*?) \| \*\*Time\*\*:.*?(?: \| \*\*Message ID\*\*: ([^\n]+))?$",
    )
    .expect("valid conversation header regex");

    normalized
        .split("\n---\n")
        .filter_map(|chunk| {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                return None;
            }

            let (header, body) = chunk.split_once("\n\n")?;
            let caps = header_re.captures(header.trim())?;
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
    let mention_only = Regex::new(r"^(?:<@!?\d+>\s*)+$").expect("valid mention regex");
    mention_only.is_match(body.trim())
}

pub(crate) fn collect_pending_workset(full_context: &str, trigger_id: Option<&str>) -> String {
    let entries = parse_conversation_entries(full_context);
    if entries.is_empty() {
        return full_context.trim().to_string();
    }

    let trigger_index = trigger_id
        .and_then(|id| {
            entries
                .iter()
                .rposition(|entry| entry.message_id.as_deref() == Some(id))
        })
        .unwrap_or_else(|| entries.len().saturating_sub(1));

    let start_index = entries[..trigger_index]
        .iter()
        .rposition(|entry| entry.author.contains("Tellar"))
        .map(|index| index + 1)
        .unwrap_or(0);

    let pending_messages: Vec<String> = entries[start_index..=trigger_index]
        .iter()
        .filter(|entry| !entry.author.contains("Tellar"))
        .filter(|entry| !entry.body.is_empty())
        .filter(|entry| !is_wake_only_message(&entry.body))
        .map(|entry| entry.body.clone())
        .collect();

    pending_messages.join("\n\n")
}

fn extract_json_object(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        return Ok(trimmed.to_string());
    }

    let fenced = Regex::new(r"(?s)```(?:json)?\s*(\{.*\})\s*```")
        .expect("valid fenced json regex");
    if let Some(caps) = fenced.captures(trimmed) {
        if let Some(body) = caps.get(1) {
            return Ok(body.as_str().trim().to_string());
        }
    }

    bail!("model output did not contain a JSON object")
}

fn has_host_absolute_path(text: &str) -> bool {
    let host_path = Regex::new(r#"(^|[\s`'"(])/(?:[^/\s]+/)*[^/\s]+"#).expect("valid host path regex");
    host_path.is_match(text)
}

fn collect_tool_specs(base_path: &Path, config: &Config, text: &str) -> (HashSet<String>, Value) {
    let mut tool_specs = get_routing_tool_definitions(base_path);
    if has_host_absolute_path(text) {
        let allowed_host_tools = if config.runtime.privileged {
            ["exec", "send_attachment", "send_attachments"]
        } else {
            ["exec", "send_attachment", "send_attachments"]
        };
        let filtered = tool_specs
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| {
                entry.get("name")
                    .and_then(Value::as_str)
                    .map(|name| allowed_host_tools.contains(&name))
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();
        tool_specs = Value::Array(filtered);
    }

    let allowed_tools = tool_specs
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect::<HashSet<_>>();

    (allowed_tools, tool_specs)
}

fn validate_tool_args(raw: Option<Value>) -> Value {
    match raw {
        Some(Value::Object(map)) => Value::Object(map),
        Some(Value::Null) | None => Value::Object(Map::new()),
        Some(other) => json!({ "value": other }),
    }
}

fn validate_route_decision(
    decision: RouteDecision,
    allowed_tools: &HashSet<String>,
) -> Result<RequestRoute> {
    match decision.route.trim().to_ascii_lowercase().as_str() {
        "agent" => Ok(RequestRoute::Agent),
        "reject" => {
            let reason = decision
                .reason
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .context("reject route requires a non-empty reason")?;
            Ok(RequestRoute::Reject { reason })
        }
        "plan" | "planandexecute" => {
            if decision.steps.is_empty() {
                bail!("plan route requires at least one step");
            }

            let mut steps = Vec::with_capacity(decision.steps.len());
            for step in decision.steps {
                let kind = step.kind.trim().to_ascii_lowercase();
                match kind.as_str() {
                    "calltool" | "call_tool" => {
                        let tool_name = step
                            .tool_name
                            .map(|v| v.trim().to_string())
                            .filter(|v| !v.is_empty())
                            .context("CallTool step requires tool_name")?;
                        if !allowed_tools.contains(&tool_name) {
                            bail!("CallTool references unknown tool `{}`", tool_name);
                        }
                        steps.push(PlanStep::CallTool {
                            tool_name,
                            args: validate_tool_args(step.args),
                        });
                    }
                    "respond" => {
                        let instruction = step
                            .instruction
                            .map(|v| v.trim().to_string())
                            .filter(|v| !v.is_empty())
                            .context("Respond step requires instruction")?;
                        steps.push(PlanStep::Respond { instruction });
                    }
                    "askformissing" | "ask_for_missing" => {
                        let prompt = step
                            .prompt
                            .map(|v| v.trim().to_string())
                            .filter(|v| !v.is_empty())
                            .context("AskForMissing step requires prompt")?;
                        steps.push(PlanStep::AskForMissing { prompt });
                    }
                    other => bail!("unsupported plan step kind `{}`", other),
                }
            }

            Ok(RequestRoute::PlanAndExecute { steps })
        }
        other => bail!("unsupported route `{}`", other),
    }
}

pub(crate) async fn plan_conversational_request(
    base_path: &Path,
    config: &Config,
    text: &str,
) -> Result<RequestRoute> {
    let (allowed_tools, tool_specs) = collect_tool_specs(base_path, config, text);

    let routing_prompt = format!(
        "You are Tellar's conversational router. Return exactly one JSON object and nothing else.\n\
Classify the user request into one of three routes:\n\
- \"plan\": for high-confidence deterministic handling using a finite plan\n\
- \"reject\": for clearly unsupported requests that should be declined directly\n\
- \"agent\": for ambiguous, open-ended, or exploratory requests that should go to the general agent loop\n\n\
When route is \"plan\", every step must be one of:\n\
- CallTool: requires tool_name and args (args must be a JSON object)\n\
- Respond: requires instruction\n\
- AskForMissing: requires prompt\n\n\
Rules:\n\
- Use only tools from the catalog below.\n\
- Prefer \"plan\" for explicit tool requests or clear, narrow requests that map cleanly to one tool.\n\
- Use AskForMissing when a deterministic tool is implied but required inputs are missing.\n\
- Use Respond for plain conversational replies or post-tool commentary.\n\
- Use Reject only when the request clearly asks for unsupported real-time external data or another capability not present in the tool catalog.\n\
- If the request references an absolute host path such as /root/... or /var/..., do not choose guild-scoped file tools. Use only host-capable tools from the catalog.\n\
- Use Agent when the request is genuinely ambiguous or requires exploration.\n\n\
Tool catalog:\n{}\n\n\
Output schema:\n\
{{\"route\":\"plan|reject|agent\",\"reason\":\"... only for reject\",\"steps\":[{{\"kind\":\"CallTool|Respond|AskForMissing\",...}}]}}",
        serde_json::to_string_pretty(&tool_specs).unwrap_or_else(|_| "[]".to_string())
    );

    let user_prompt = format!("Route this request:\n{}", text);

    let turn = llm::generate_turn(
        &routing_prompt,
        vec![llm::Message {
            role: llm::MessageRole::User,
            parts: vec![llm::MultimodalPart::text(user_prompt)],
        }],
        &config.gemini.api_key,
        &config.gemini.model,
        0.1,
        None,
    )
    .await?;

    let narrative = match turn {
        llm::ModelTurn::Narrative(text) => text,
        llm::ModelTurn::ToolCalls { .. } => bail!("routing model attempted tool calls"),
    };

    let json_payload = extract_json_object(&narrative)?;
    let decision: RouteDecision = serde_json::from_str(&json_payload)
        .with_context(|| format!("invalid router JSON: {}", json_payload))?;

    validate_route_decision(decision, &allowed_tools)
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

    fn tool_set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn test_validate_route_decision_accepts_tool_plan() {
        let decision = RouteDecision {
            route: "plan".to_string(),
            reason: None,
            steps: vec![RouteDecisionStep {
                kind: "CallTool".to_string(),
                tool_name: Some("stock_quote".to_string()),
                args: Some(json!({ "symbol": "TSLA.US" })),
                instruction: None,
                prompt: None,
            }],
        };

        let route = validate_route_decision(decision, &tool_set(&["stock_quote"])).unwrap();
        match route {
            RequestRoute::PlanAndExecute { steps } => {
                assert_eq!(steps.len(), 1);
                assert!(matches!(steps[0], PlanStep::CallTool { .. }));
            }
            _ => panic!("expected plan route"),
        }
    }

    #[test]
    fn test_validate_route_decision_accepts_ask_for_missing() {
        let decision = RouteDecision {
            route: "plan".to_string(),
            reason: None,
            steps: vec![RouteDecisionStep {
                kind: "AskForMissing".to_string(),
                tool_name: None,
                args: None,
                instruction: None,
                prompt: Some("Need expiry".to_string()),
            }],
        };

        let route = validate_route_decision(decision, &tool_set(&[])).unwrap();
        match route {
            RequestRoute::PlanAndExecute { steps } => {
                assert!(matches!(steps[0], PlanStep::AskForMissing { .. }));
            }
            _ => panic!("expected plan route"),
        }
    }

    #[test]
    fn test_validate_route_decision_accepts_reject() {
        let decision = RouteDecision {
            route: "reject".to_string(),
            reason: Some("No weather tool".to_string()),
            steps: vec![],
        };

        let route = validate_route_decision(decision, &tool_set(&[])).unwrap();
        match route {
            RequestRoute::Reject { reason } => assert!(reason.contains("weather")),
            _ => panic!("expected reject route"),
        }
    }

    #[test]
    fn test_validate_route_decision_rejects_unknown_tool() {
        let decision = RouteDecision {
            route: "plan".to_string(),
            reason: None,
            steps: vec![RouteDecisionStep {
                kind: "CallTool".to_string(),
                tool_name: Some("unknown_tool".to_string()),
                args: Some(json!({})),
                instruction: None,
                prompt: None,
            }],
        };

        assert!(validate_route_decision(decision, &tool_set(&["stock_quote"])).is_err());
    }

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
        assert_eq!(extracted, "益阳天气如何？");
    }

    #[test]
    fn test_collect_pending_workset_preserves_single_message_mode() {
        let content = concat!(
            "---\n**Author**: Dagow (ID: 1) | **Time**: t1 | **Message ID**: only\n\n",
            "看下 TSLA 的股价\n",
        );

        let extracted = collect_pending_workset(content, Some("only"));
        assert_eq!(extracted, "看下 TSLA 的股价");
    }

    #[test]
    fn test_collect_tool_specs_reads_installed_skills() {
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

        let config = Config {
            gemini: crate::config::GeminiConfig {
                api_key: "fake".to_string(),
                model: "fake".to_string(),
            },
            discord: crate::config::DiscordConfig {
                token: "fake".to_string(),
                guild_id: None,
                channel_mappings: None,
            },
            runtime: crate::config::RuntimeConfig::default(),
            guardian: None,
        };

        let (allowed, specs) = collect_tool_specs(dir.path(), &config, "看下 TSLA.US 的股价");
        assert!(allowed.contains("ls"));
        assert!(allowed.contains("send_attachment"));
        assert!(allowed.contains("stock_quote"));
        assert!(specs.as_array().unwrap().iter().any(|entry| entry["name"] == "ls"));
        assert!(specs
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "send_attachment"));
        assert!(specs
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "stock_quote"));
    }

    #[test]
    fn test_collect_tool_specs_limits_host_path_requests_to_host_capable_tools() {
        let dir = tempdir().unwrap();
        let config = Config {
            gemini: crate::config::GeminiConfig {
                api_key: "fake".to_string(),
                model: "fake".to_string(),
            },
            discord: crate::config::DiscordConfig {
                token: "fake".to_string(),
                guild_id: None,
                channel_mappings: None,
            },
            runtime: crate::config::RuntimeConfig {
                max_turns: 16,
                read_only_budget: 4,
                max_tool_output_bytes: 5000,
                privileged: true,
                exec_mode: crate::config::ExecMode::Unrestricted,
            },
            guardian: None,
        };

        let (allowed, specs) =
            collect_tool_specs(dir.path(), &config, "找到 /root/process_intel.py 文件，并且发给我");
        assert!(allowed.contains("exec"));
        assert!(allowed.contains("send_attachment"));
        assert!(!allowed.contains("find"));
        assert!(!allowed.contains("ls"));
        assert!(specs
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| matches!(entry["name"].as_str(), Some("exec" | "send_attachment" | "send_attachments"))));
    }
}
