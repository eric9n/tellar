/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/task_policy.rs
 * Responsibility: Task-specific execution boundaries and routing guardrails.
 */

use crate::execution_contract::{PlanConfidence, RequestRoute};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RoutePolicyDecision {
    pub(crate) route: RequestRoute,
    pub(crate) converted_low_confidence_to_needs_input: bool,
}

impl RoutePolicyDecision {
    pub(crate) fn log_note(&self) -> Option<&'static str> {
        if self.converted_low_confidence_to_needs_input {
            Some("Low-confidence plan converted to a clarification request.")
        } else {
            None
        }
    }
}

pub(crate) fn apply_request_route_policy(route: RequestRoute) -> RoutePolicyDecision {
    match route {
        RequestRoute::PlanAndExecute { plan } if plan.confidence == PlanConfidence::Low => {
            RoutePolicyDecision {
                route: RequestRoute::NeedsInput {
                    fields: Vec::new(),
                    prompt: Some(
                        "This task is not ready to execute. Provide the exact target or required inputs."
                            .to_string(),
                    ),
                },
                converted_low_confidence_to_needs_input: true,
            }
        }
        other => RoutePolicyDecision {
            route: other,
            converted_low_confidence_to_needs_input: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_request_route_policy_downgrades_low_confidence_plan() {
        let route = RequestRoute::PlanAndExecute {
            plan: crate::execution_contract::ExecutionPlan {
                intent: crate::execution_contract::PlanIntent::ToolExecution,
                confidence: crate::execution_contract::PlanConfidence::Low,
                steps: vec![],
            },
        };

        let decision = apply_request_route_policy(route);

        assert!(matches!(decision.route, RequestRoute::NeedsInput { .. }));
        assert_eq!(
            decision.log_note(),
            Some("Low-confidence plan converted to a clarification request.")
        );
        assert!(decision.converted_low_confidence_to_needs_input);
    }

    #[test]
    fn test_apply_request_route_policy_keeps_non_low_plan() {
        let route = RequestRoute::PlanAndExecute {
            plan: crate::execution_contract::ExecutionPlan {
                intent: crate::execution_contract::PlanIntent::ToolExecution,
                confidence: crate::execution_contract::PlanConfidence::High,
                steps: vec![],
            },
        };

        let decision = apply_request_route_policy(route);

        assert!(matches!(
            decision.route,
            RequestRoute::PlanAndExecute { .. }
        ));
        assert_eq!(decision.log_note(), None);
        assert!(!decision.converted_low_confidence_to_needs_input);
    }
}
