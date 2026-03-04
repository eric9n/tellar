/*
 * Tellar - Minimal Document-Driven Cyber Steward
 * File Path: src/execution_contract.rs
 * Responsibility: Shared task routing and execution contracts.
 */

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolCallSpec {
    pub(crate) tool_name: String,
    pub(crate) args: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanIntent {
    DirectResponse,
    ToolExecution,
    ToolExecutionWithResponse,
    MissingInputCollection,
}

impl PlanIntent {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::DirectResponse => "DirectResponse",
            Self::ToolExecution => "ToolExecution",
            Self::ToolExecutionWithResponse => "ToolExecutionWithResponse",
            Self::MissingInputCollection => "MissingInputCollection",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanConfidence {
    High,
    Medium,
    Low,
}

impl PlanConfidence {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::High => "High",
            Self::Medium => "Medium",
            Self::Low => "Low",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponseStyle {
    Direct,
    BriefCommentary,
    Summary,
}

impl ResponseStyle {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Direct => "Direct",
            Self::BriefCommentary => "BriefCommentary",
            Self::Summary => "Summary",
        }
    }

    pub(crate) fn instruction(self) -> &'static str {
        match self {
            Self::Direct => {
                "Return a direct user-facing answer using the tool result. Do not call tools."
            }
            Self::BriefCommentary => {
                "Return a brief user-facing answer with a concise commentary. Do not call tools."
            }
            Self::Summary => {
                "Summarize the result for the user in a concise way. Do not call tools."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PlanStep {
    CallTool {
        call: ToolCallSpec,
    },
    Respond {
        style: ResponseStyle,
        guidance: Option<String>,
    },
    AskForMissing {
        fields: Vec<String>,
        prompt: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExecutionPlan {
    pub(crate) intent: PlanIntent,
    pub(crate) confidence: PlanConfidence,
    pub(crate) steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecutionStepKind {
    Rejected {
        reason: String,
    },
    CalledTool {
        tool_name: String,
        succeeded: bool,
    },
    Responded {
        style: ResponseStyle,
    },
    RespondFallback {
        style: ResponseStyle,
    },
    RequestedMissingInput {
        fields: Vec<String>,
        prompt_only: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutionStepOutcome {
    Completed,
    Error,
    Fallback,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionStepView {
    pub(crate) index: usize,
    pub(crate) label: &'static str,
    pub(crate) outcome: ExecutionStepOutcome,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionTraceView {
    pub(crate) intent: PlanIntent,
    pub(crate) confidence: PlanConfidence,
    pub(crate) steps: Vec<ExecutionStepView>,
}

impl ExecutionTraceView {
    pub(crate) fn summarize(&self) -> String {
        let summary = self
            .steps
            .iter()
            .map(|step| {
                format!(
                    "#{} {}:{}({})",
                    step.index,
                    step.label,
                    step.outcome.as_str(),
                    step.detail
                )
            })
            .collect::<Vec<_>>()
            .join(" -> ");
        format!(
            "intent={} confidence={} steps=[{}]",
            self.intent.label(),
            self.confidence.label(),
            summary
        )
    }
}

impl ExecutionStepKind {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Rejected { .. } => "Reject",
            Self::CalledTool { .. } => "CallTool",
            Self::Responded { .. } => "Respond",
            Self::RespondFallback { .. } => "Respond",
            Self::RequestedMissingInput { .. } => "AskForMissing",
        }
    }

    pub(crate) fn outcome(&self) -> ExecutionStepOutcome {
        match self {
            Self::Rejected { .. } => ExecutionStepOutcome::Terminal,
            Self::CalledTool { succeeded, .. } => {
                if *succeeded {
                    ExecutionStepOutcome::Completed
                } else {
                    ExecutionStepOutcome::Error
                }
            }
            Self::Responded { .. } => ExecutionStepOutcome::Terminal,
            Self::RespondFallback { .. } => ExecutionStepOutcome::Fallback,
            Self::RequestedMissingInput { .. } => ExecutionStepOutcome::Terminal,
        }
    }

    pub(crate) fn detail(&self) -> String {
        match self {
            Self::Rejected { reason } => reason.clone(),
            Self::CalledTool { tool_name, .. } => tool_name.clone(),
            Self::Responded { style } => style.label().to_string(),
            Self::RespondFallback { style } => format!("{}:fallback", style.label()),
            Self::RequestedMissingInput {
                fields,
                prompt_only,
            } => {
                if *prompt_only {
                    "prompt-only".to_string()
                } else {
                    fields.join("+")
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionStepTrace {
    pub(crate) index: usize,
    pub(crate) step: ExecutionStepKind,
}

impl ExecutionStepTrace {
    pub(crate) fn view(&self) -> ExecutionStepView {
        ExecutionStepView {
            index: self.index,
            label: self.step.label(),
            outcome: self.step.outcome(),
            detail: self.step.detail(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionTrace {
    pub(crate) intent: PlanIntent,
    pub(crate) confidence: PlanConfidence,
    pub(crate) steps: Vec<ExecutionStepTrace>,
}

impl ExecutionTrace {
    pub(crate) fn view(&self) -> ExecutionTraceView {
        ExecutionTraceView {
            intent: self.intent,
            confidence: self.confidence,
            steps: self.steps.iter().map(ExecutionStepTrace::view).collect(),
        }
    }

    pub(crate) fn summarize(&self) -> String {
        self.view().summarize()
    }
}

impl ExecutionStepOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Error => "Error",
            Self::Fallback => "Fallback",
            Self::Terminal => "Terminal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutionFinalState {
    Completed,
    Rejected,
    Failed,
    NeedsInput,
}

impl ExecutionFinalState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Rejected => "Rejected",
            Self::Failed => "Failed",
            Self::NeedsInput => "NeedsInput",
        }
    }

    pub(crate) fn is_terminal_success(self) -> bool {
        matches!(self, Self::Completed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionOutcome {
    pub(crate) final_state: ExecutionFinalState,
    pub(crate) user_response: String,
    pub(crate) trace: ExecutionTrace,
}

impl ExecutionOutcome {
    pub(crate) fn is_terminal_success(&self) -> bool {
        self.final_state.is_terminal_success()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConversationalLoopState {
    NoNewWorkset,
    Planned(ExecutionFinalState),
}

impl ConversationalLoopState {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::NoNewWorkset => "NoNewWorkset",
            Self::Planned(_) => "Planned",
        }
    }

    pub(crate) fn planned_final_state(&self) -> Option<ExecutionFinalState> {
        match self {
            Self::Planned(final_state) => Some(*final_state),
            Self::NoNewWorkset => None,
        }
    }

    pub(crate) fn is_planned(&self) -> bool {
        self.planned_final_state().is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConversationalLoopOutcome {
    pub(crate) user_response: String,
    pub(crate) state: ConversationalLoopState,
    pub(crate) trace: Option<ExecutionTraceView>,
}

impl ConversationalLoopOutcome {
    pub(crate) fn trace_summary(&self) -> Option<String> {
        self.trace.as_ref().map(ExecutionTraceView::summarize)
    }

    pub(crate) fn log_summary(&self) -> String {
        if self.state.is_planned() {
            let final_state = self
                .state
                .planned_final_state()
                .expect("planned state should carry a final state");
            if let Some(trace_summary) = self.trace_summary() {
                format!("{} {}", final_state.label(), trace_summary)
            } else {
                final_state.label().to_string()
            }
        } else {
            self.state.label().to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RequestRoute {
    PlanAndExecute {
        plan: ExecutionPlan,
    },
    NeedsInput {
        fields: Vec<String>,
        prompt: Option<String>,
    },
    Reject {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExecutableRoute {
    PlanAndExecute { plan: ExecutionPlan },
    Reject { reason: String },
}

impl RequestRoute {
    pub(crate) fn into_executable(self) -> ExecutableRoute {
        match self {
            RequestRoute::PlanAndExecute { plan } => ExecutableRoute::PlanAndExecute { plan },
            RequestRoute::NeedsInput { fields, prompt } => ExecutableRoute::PlanAndExecute {
                plan: ExecutionPlan {
                    intent: PlanIntent::MissingInputCollection,
                    confidence: PlanConfidence::High,
                    steps: vec![PlanStep::AskForMissing { fields, prompt }],
                },
            },
            RequestRoute::Reject { reason } => ExecutableRoute::Reject { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_step_kind_exposes_stable_summary_fields() {
        let step = ExecutionStepKind::RespondFallback {
            style: ResponseStyle::BriefCommentary,
        };

        assert_eq!(ResponseStyle::BriefCommentary.label(), "BriefCommentary");
        assert_eq!(
            ResponseStyle::Summary.instruction(),
            "Summarize the result for the user in a concise way. Do not call tools."
        );
        assert_eq!(step.label(), "Respond");
        assert_eq!(step.outcome(), ExecutionStepOutcome::Fallback);
        assert_eq!(step.outcome().as_str(), "Fallback");
        assert_eq!(step.detail(), "BriefCommentary:fallback");
    }

    #[test]
    fn execution_trace_summarize_uses_structured_step_accessors() {
        let trace = ExecutionTrace {
            intent: PlanIntent::ToolExecution,
            confidence: PlanConfidence::High,
            steps: vec![
                ExecutionStepTrace {
                    index: 1,
                    step: ExecutionStepKind::CalledTool {
                        tool_name: "ls".to_string(),
                        succeeded: true,
                    },
                },
                ExecutionStepTrace {
                    index: 2,
                    step: ExecutionStepKind::Responded {
                        style: ResponseStyle::Summary,
                    },
                },
            ],
        };

        assert_eq!(
            trace.summarize(),
            "intent=ToolExecution confidence=High steps=[#1 CallTool:Completed(ls) -> #2 Respond:Terminal(Summary)]"
        );
    }

    #[test]
    fn execution_trace_view_exposes_structured_export() {
        let trace = ExecutionTrace {
            intent: PlanIntent::MissingInputCollection,
            confidence: PlanConfidence::Medium,
            steps: vec![ExecutionStepTrace {
                index: 1,
                step: ExecutionStepKind::RequestedMissingInput {
                    fields: vec!["location".to_string()],
                    prompt_only: false,
                },
            }],
        };

        let view = trace.view();

        assert_eq!(view.intent, PlanIntent::MissingInputCollection);
        assert_eq!(view.confidence, PlanConfidence::Medium);
        assert_eq!(view.steps.len(), 1);
        assert_eq!(view.steps[0].index, 1);
        assert_eq!(view.steps[0].label, "AskForMissing");
        assert_eq!(view.steps[0].outcome, ExecutionStepOutcome::Terminal);
        assert_eq!(view.steps[0].detail, "location");
        assert_eq!(
            view.summarize(),
            "intent=MissingInputCollection confidence=Medium steps=[#1 AskForMissing:Terminal(location)]"
        );
    }

    #[test]
    fn conversational_loop_outcome_trace_summary_uses_view() {
        let outcome = ConversationalLoopOutcome {
            user_response: "Need `location` before I can continue.".to_string(),
            state: ConversationalLoopState::Planned(ExecutionFinalState::NeedsInput),
            trace: Some(ExecutionTraceView {
                intent: PlanIntent::MissingInputCollection,
                confidence: PlanConfidence::High,
                steps: vec![ExecutionStepView {
                    index: 1,
                    label: "AskForMissing",
                    outcome: ExecutionStepOutcome::Terminal,
                    detail: "location".to_string(),
                }],
            }),
        };

        assert_eq!(
            outcome.trace_summary(),
            Some(
                "intent=MissingInputCollection confidence=High steps=[#1 AskForMissing:Terminal(location)]"
                .to_string()
            )
        );
    }

    #[test]
    fn conversational_loop_outcome_log_summary_includes_state_and_trace() {
        let outcome = ConversationalLoopOutcome {
            user_response: "Need `location` before I can continue.".to_string(),
            state: ConversationalLoopState::Planned(ExecutionFinalState::NeedsInput),
            trace: Some(ExecutionTraceView {
                intent: PlanIntent::MissingInputCollection,
                confidence: PlanConfidence::High,
                steps: vec![ExecutionStepView {
                    index: 1,
                    label: "AskForMissing",
                    outcome: ExecutionStepOutcome::Terminal,
                    detail: "location".to_string(),
                }],
            }),
        };

        assert_eq!(
            outcome.log_summary(),
            "NeedsInput intent=MissingInputCollection confidence=High steps=[#1 AskForMissing:Terminal(location)]"
        );
    }

    #[test]
    fn conversational_loop_state_exposes_stable_accessors() {
        let planned = ConversationalLoopState::Planned(ExecutionFinalState::Completed);

        assert_eq!(planned.label(), "Planned");
        assert_eq!(
            planned.planned_final_state(),
            Some(ExecutionFinalState::Completed)
        );
        assert!(planned.is_planned());
    }

    #[test]
    fn execution_final_state_exposes_stable_accessors() {
        assert_eq!(ExecutionFinalState::Completed.label(), "Completed");
        assert_eq!(ExecutionFinalState::Rejected.label(), "Rejected");
        assert!(ExecutionFinalState::Completed.is_terminal_success());
        assert!(!ExecutionFinalState::Failed.is_terminal_success());
        assert!(!ExecutionFinalState::NeedsInput.is_terminal_success());
    }

    #[test]
    fn plan_metadata_exposes_stable_accessors() {
        assert_eq!(
            PlanIntent::ToolExecutionWithResponse.label(),
            "ToolExecutionWithResponse"
        );
        assert_eq!(
            PlanIntent::MissingInputCollection.label(),
            "MissingInputCollection"
        );
        assert_eq!(PlanConfidence::High.label(), "High");
        assert_eq!(PlanConfidence::Low.label(), "Low");
    }
}
