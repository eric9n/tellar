/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/router.rs
 * Responsibility: LLM-backed task routing that converts requests into deterministic execution paths,
 * then validates the structured routing decision.
 */

use crate::config::Config;
use crate::execution_contract::{
    ExecutionPlan, PlanConfidence, PlanIntent, PlanStep, RequestRoute, ResponseStyle, ToolCallSpec,
};
use crate::input::Workset;
use crate::llm;
use crate::routing_catalog::collect_routing_tool_catalog;
use anyhow::{Context, Result, bail};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::path::Path;

static FENCED_JSON_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)```(?:json)?\s*(\{.*\})\s*```").expect("valid fenced json regex")
});

#[derive(Debug, Deserialize)]
struct RouteDecision {
    route: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    fields: Vec<String>,
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    confidence: Option<String>,
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
    style: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    fields: Vec<String>,
}

fn extract_json_object(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        return Ok(trimmed.to_string());
    }

    if let Some(caps) = FENCED_JSON_RE.captures(trimmed) {
        if let Some(body) = caps.get(1) {
            return Ok(body.as_str().trim().to_string());
        }
    }

    bail!("model output did not contain a JSON object")
}

fn validate_tool_args(raw: Option<Value>) -> Value {
    match raw {
        Some(Value::Object(map)) => Value::Object(map),
        Some(Value::Null) | None => Value::Object(Map::new()),
        Some(other) => json!({ "value": other }),
    }
}

fn infer_plan_intent(steps: &[PlanStep]) -> PlanIntent {
    let has_tool = steps
        .iter()
        .any(|step| matches!(step, PlanStep::CallTool { .. }));
    let has_respond = steps
        .iter()
        .any(|step| matches!(step, PlanStep::Respond { .. }));
    let has_missing = steps
        .iter()
        .any(|step| matches!(step, PlanStep::AskForMissing { .. }));

    if has_missing && !has_tool && !has_respond {
        PlanIntent::MissingInputCollection
    } else if has_tool && has_respond {
        PlanIntent::ToolExecutionWithResponse
    } else if has_tool {
        PlanIntent::ToolExecution
    } else {
        PlanIntent::DirectResponse
    }
}

fn parse_plan_intent(raw: Option<String>, steps: &[PlanStep]) -> Result<PlanIntent> {
    let Some(value) = raw else {
        return Ok(infer_plan_intent(steps));
    };

    match value.trim().to_ascii_lowercase().as_str() {
        "direct_response" | "direct-response" | "direct" => Ok(PlanIntent::DirectResponse),
        "tool_execution" | "tool-execution" | "tool" => Ok(PlanIntent::ToolExecution),
        "tool_execution_with_response"
        | "tool-execution-with-response"
        | "tool_with_response"
        | "tool-with-response" => Ok(PlanIntent::ToolExecutionWithResponse),
        "missing_input_collection"
        | "missing-input-collection"
        | "ask_for_missing"
        | "ask-for-missing"
        | "missing" => Ok(PlanIntent::MissingInputCollection),
        other => bail!("unsupported plan intent `{}`", other),
    }
}

fn parse_plan_confidence(raw: Option<String>) -> Result<PlanConfidence> {
    let Some(value) = raw else {
        return Ok(PlanConfidence::High);
    };

    match value.trim().to_ascii_lowercase().as_str() {
        "high" => Ok(PlanConfidence::High),
        "medium" => Ok(PlanConfidence::Medium),
        "low" => Ok(PlanConfidence::Low),
        other => bail!("unsupported plan confidence `{}`", other),
    }
}

fn validate_route_decision(
    decision: RouteDecision,
    allowed_tools: &HashSet<String>,
) -> Result<RequestRoute> {
    let RouteDecision {
        route,
        reason,
        prompt,
        fields,
        intent,
        confidence,
        steps: raw_steps,
    } = decision;

    match route.trim().to_ascii_lowercase().as_str() {
        "needs_input" | "needsinput" | "clarify" => {
            let prompt = prompt
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            let fields = fields
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            if prompt.is_none() && fields.is_empty() {
                bail!("needs_input route requires prompt or fields");
            }
            Ok(RequestRoute::NeedsInput { fields, prompt })
        }
        "reject" => {
            let reason = reason
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .context("reject route requires a non-empty reason")?;
            Ok(RequestRoute::Reject { reason })
        }
        "plan" | "planandexecute" => {
            if raw_steps.is_empty() {
                bail!("plan route requires at least one step");
            }

            let mut steps = Vec::with_capacity(raw_steps.len());
            for step in raw_steps {
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
                            call: ToolCallSpec {
                                tool_name,
                                args: validate_tool_args(step.args),
                            },
                        });
                    }
                    "respond" => {
                        let guidance = step
                            .instruction
                            .map(|v| v.trim().to_string())
                            .filter(|v| !v.is_empty());
                        let style = match step
                            .style
                            .as_deref()
                            .unwrap_or("summary")
                            .trim()
                            .to_ascii_lowercase()
                            .as_str()
                        {
                            "direct" => ResponseStyle::Direct,
                            "brief_commentary" | "brief-commentary" | "commentary" => {
                                ResponseStyle::BriefCommentary
                            }
                            "summary" => ResponseStyle::Summary,
                            other => bail!("unsupported respond style `{}`", other),
                        };
                        steps.push(PlanStep::Respond { style, guidance });
                    }
                    "askformissing" | "ask_for_missing" => {
                        let prompt = step
                            .prompt
                            .map(|v| v.trim().to_string())
                            .filter(|v| !v.is_empty());
                        let fields = step
                            .fields
                            .into_iter()
                            .map(|v| v.trim().to_string())
                            .filter(|v| !v.is_empty())
                            .collect::<Vec<_>>();
                        if prompt.is_none() && fields.is_empty() {
                            bail!("AskForMissing step requires prompt or fields");
                        }
                        steps.push(PlanStep::AskForMissing { fields, prompt });
                    }
                    other => bail!("unsupported plan step kind `{}`", other),
                }
            }

            let intent = parse_plan_intent(intent, &steps)?;
            let confidence = parse_plan_confidence(confidence)?;

            Ok(RequestRoute::PlanAndExecute {
                plan: ExecutionPlan {
                    intent,
                    confidence,
                    steps,
                },
            })
        }
        other => bail!("unsupported route `{}`", other),
    }
}

pub(crate) fn parse_route_decision(
    narrative: &str,
    allowed_tools: &HashSet<String>,
) -> Result<RequestRoute> {
    let json_payload = extract_json_object(narrative)?;
    let decision: RouteDecision = serde_json::from_str(&json_payload)
        .with_context(|| format!("invalid router JSON: {}", json_payload))?;
    validate_route_decision(decision, allowed_tools)
}

pub(crate) async fn plan_conversational_request(
    base_path: &Path,
    config: &Config,
    workset: &Workset,
) -> Result<RequestRoute> {
    let text = workset.text();
    let catalog = collect_routing_tool_catalog(base_path, config, &text);
    let allowed_tools = &catalog.allowed_tools;

    let routing_prompt = format!(
        "You are Tellar's task router. Return exactly one JSON object and nothing else.\n\
Your job is to identify the user's task intent, decide whether the task is executable, and produce the narrowest safe route.\n\
Tellar is a task processor, not a chat companion. Do not optimize for conversational variety. Optimize for precise execution.\n\
Classify the user request into one of three routes:\n\
- \"plan\": for high-confidence deterministic handling using a finite plan\n\
- \"needs_input\": when the request seems feasible but required inputs or scope are missing\n\
- \"reject\": for clearly unsupported requests that should be declined directly\n\
\n\
When route is \"plan\", every step must be one of:\n\
- CallTool: requires tool_name and args (args must be a JSON object)\n\
- Respond: optional style (direct | brief_commentary | summary), optional instruction\n\
- AskForMissing: requires at least one of fields (array of missing field names) or prompt\n\n\
When route is \"needs_input\", include at least one of:\n\
- fields: array of missing field names\n\
- prompt: a direct clarification question to the user\n\n\
Rules:\n\
- Use only tools from the catalog below.\n\
- Prefer \"plan\" for explicit task requests or clear, narrow requests that map cleanly to one tool.\n\
- Use \"needs_input\" when a deterministic tool is implied but required inputs are missing.\n\
- Use Respond only for final task output or concise post-tool delivery.\n\
- Use Reject only when the task cannot be completed with the available capabilities.\n\
- If the request references an absolute host path such as /root/... or /var/..., do not choose guild-scoped file tools. Use only host-capable tools from the catalog.\n\
- If the request is too ambiguous to execute safely, prefer \"needs_input\" over \"reject\".\n\n\
Optional plan metadata:\n\
- intent: direct_response | tool_execution | tool_execution_with_response | missing_input_collection\n\
- confidence: high | medium | low\n\
- If omitted, the system will infer sensible defaults.\n\n\
Tool catalog:\n{}\n\n\
Output schema:\n\
{{\"route\":\"plan|needs_input|reject\",\"intent\":\"... optional for plan\",\"confidence\":\"... optional for plan\",\"reason\":\"... only for reject\",\"prompt\":\"... for needs_input\",\"fields\":[\"...\"],\"steps\":[{{\"kind\":\"CallTool|Respond|AskForMissing\",...}}]}}",
        catalog.rendered_specs
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

    parse_route_decision(&narrative, allowed_tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn test_parse_route_decision_accepts_tool_plan() {
        let route = parse_route_decision(
            r#"{
                "route":"plan",
                "steps":[{"kind":"CallTool","tool_name":"stock_quote","args":{"symbol":"TSLA.US"}}]
            }"#,
            &tool_set(&["stock_quote"]),
        )
        .unwrap();

        match route {
            RequestRoute::PlanAndExecute { plan } => {
                assert_eq!(plan.steps.len(), 1);
                assert_eq!(plan.intent, PlanIntent::ToolExecution);
                assert_eq!(plan.confidence, PlanConfidence::High);
                assert!(matches!(plan.steps[0], PlanStep::CallTool { .. }));
            }
            _ => panic!("expected plan route"),
        }
    }

    #[test]
    fn test_parse_route_decision_accepts_ask_for_missing() {
        let route = parse_route_decision(
            r#"{
                "route":"plan",
                "steps":[{"kind":"AskForMissing","fields":["expiry"]}]
            }"#,
            &tool_set(&[]),
        )
        .unwrap();

        match route {
            RequestRoute::PlanAndExecute { plan } => {
                assert_eq!(plan.intent, PlanIntent::MissingInputCollection);
                assert!(matches!(plan.steps[0], PlanStep::AskForMissing { .. }));
            }
            _ => panic!("expected plan route"),
        }
    }

    #[test]
    fn test_parse_route_decision_accepts_structured_respond() {
        let route = parse_route_decision(
            r#"{
                "route":"plan",
                "steps":[{"kind":"Respond","style":"brief_commentary","instruction":"Keep it short"}]
            }"#,
            &tool_set(&[]),
        )
        .unwrap();

        match route {
            RequestRoute::PlanAndExecute { plan } => {
                assert_eq!(plan.intent, PlanIntent::DirectResponse);
                assert!(matches!(
                    plan.steps[0],
                    PlanStep::Respond {
                        style: ResponseStyle::BriefCommentary,
                        ..
                    }
                ));
            }
            _ => panic!("expected plan route"),
        }
    }

    #[test]
    fn test_parse_route_decision_accepts_needs_input_route() {
        let route = parse_route_decision(
            r#"{
                "route":"needs_input",
                "fields":["symbol"],
                "prompt":"Which symbol should I use?"
            }"#,
            &tool_set(&[]),
        )
        .unwrap();

        match route {
            RequestRoute::NeedsInput { fields, prompt } => {
                assert_eq!(fields, vec!["symbol".to_string()]);
                assert_eq!(prompt.as_deref(), Some("Which symbol should I use?"));
            }
            _ => panic!("expected needs_input route"),
        }
    }

    #[test]
    fn test_parse_route_decision_accepts_reject() {
        let route = parse_route_decision(
            r#"{"route":"reject","reason":"No weather tool"}"#,
            &tool_set(&[]),
        )
        .unwrap();

        match route {
            RequestRoute::Reject { reason } => assert!(reason.contains("weather")),
            _ => panic!("expected reject route"),
        }
    }

    #[test]
    fn test_parse_route_decision_rejects_unknown_tool() {
        assert!(
            parse_route_decision(
                r#"{
                "route":"plan",
                "steps":[{"kind":"CallTool","tool_name":"unknown_tool","args":{}}]
            }"#,
                &tool_set(&["stock_quote"]),
            )
            .is_err()
        );
    }

    #[test]
    fn test_parse_route_decision_accepts_explicit_plan_metadata() {
        let route = parse_route_decision(
            r#"{
                "route":"plan",
                "intent":"tool_execution_with_response",
                "confidence":"medium",
                "steps":[
                    {"kind":"CallTool","tool_name":"stock_quote","args":{"symbol":"TSLA.US"}},
                    {"kind":"Respond","style":"brief_commentary"}
                ]
            }"#,
            &tool_set(&["stock_quote"]),
        )
        .unwrap();

        match route {
            RequestRoute::PlanAndExecute { plan } => {
                assert_eq!(plan.intent, PlanIntent::ToolExecutionWithResponse);
                assert_eq!(plan.confidence, PlanConfidence::Medium);
                assert_eq!(plan.steps.len(), 2);
            }
            _ => panic!("expected plan route"),
        }
    }
}
