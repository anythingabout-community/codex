//! Durable stage planning. Actual timestamps are assigned by the host, not the model.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum PlanStepState {
    #[default]
    Pending,
    Running,
    Reviewing,
    Completed,
    Blocked,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PlanStage {
    pub id: String,
    pub title: String,
    pub acceptance: String,
    pub assumptions: Vec<String>,
    #[ts(type = "number")]
    pub estimate_seconds: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct PlanStepDefinition {
    pub id: String,
    pub stage_id: String,
    pub title: String,
    pub acceptance: String,
    pub dependencies: Vec<String>,
    #[ts(type = "number")]
    pub estimate_seconds: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TimedPlanStep {
    pub definition: PlanStepDefinition,
    pub state: PlanStepState,
    pub owner: String,
    #[ts(type = "number")]
    pub initial_estimate_seconds: i64,
    #[ts(type = "number")]
    pub elapsed_seconds: i64,
    #[ts(type = "number | null")]
    pub started_at: Option<i64>,
    #[ts(type = "number | null")]
    pub running_since: Option<i64>,
    #[ts(type = "number | null")]
    pub submitted_at: Option<i64>,
    #[ts(type = "number | null")]
    pub completed_at: Option<i64>,
    pub evidence: Vec<String>,
}

impl TimedPlanStep {
    pub fn elapsed_at(&self, now: i64) -> i64 {
        self.elapsed_seconds.saturating_add(
            self.running_since
                .map_or(0, |start| now.saturating_sub(start).max(0)),
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum PlanReviewState {
    #[default]
    Idle,
    Due,
    Running,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ContinuousPlanning {
    /// Host-assigned identity, retained across revisions and changed for a new task.
    pub id: String,
    #[ts(type = "number")]
    pub version: i64,
    pub objective: String,
    pub acceptance: String,
    pub stages: Vec<PlanStage>,
    pub steps: Vec<TimedPlanStep>,
    pub review: PlanReviewState,
    #[ts(type = "number")]
    pub updated_at: i64,
    #[ts(type = "number")]
    pub next_review_at: i64,
    #[ts(type = "number")]
    pub estimated_completion_at: i64,
    pub reason: String,
}

/// A forecast at one host timestamp, accounting for stage gates and executor dependencies.
#[derive(Debug, PartialEq, Eq)]
pub struct PlanForecast {
    /// Step completion timestamps in the same order as `ContinuousPlanning::steps`.
    pub steps: Vec<i64>,
    pub completed_at: i64,
}

impl ContinuousPlanning {
    pub fn forecast_at(&self, now: i64) -> PlanForecast {
        let mut forecast = PlanForecast {
            steps: vec![now; self.steps.len()],
            completed_at: now,
        };
        let mut dependencies = std::collections::HashMap::new();
        let mut owners = std::collections::HashMap::new();
        for stage in &self.stages {
            let stage_start = forecast.completed_at;
            let mut expanded = false;
            for (index, step) in self
                .steps
                .iter()
                .enumerate()
                .filter(|(_, step)| step.definition.stage_id == stage.id)
            {
                expanded = true;
                let dependency_end = step
                    .definition
                    .dependencies
                    .iter()
                    .filter_map(|id| dependencies.get(id))
                    .copied()
                    .max()
                    .unwrap_or(now);
                let owner_end = owners.get(&step.owner).copied().unwrap_or(now);
                let finish = match step.state {
                    PlanStepState::Completed | PlanStepState::Cancelled => {
                        step.completed_at.unwrap_or(now)
                    }
                    PlanStepState::Pending
                    | PlanStepState::Running
                    | PlanStepState::Reviewing
                    | PlanStepState::Blocked => {
                        let remaining =
                            (step.definition.estimate_seconds - step.elapsed_at(now)).max(0);
                        stage_start
                            .max(dependency_end)
                            .max(owner_end)
                            .saturating_add(remaining)
                    }
                };
                forecast.steps[index] = finish;
                dependencies.insert(&step.definition.id, finish);
                owners.insert(&step.owner, owner_end.max(finish));
                forecast.completed_at = forecast.completed_at.max(finish);
            }
            if !expanded {
                forecast.completed_at = stage_start.saturating_add(stage.estimate_seconds);
            }
        }
        if self.steps.iter().all(|step| {
            matches!(
                step.state,
                PlanStepState::Completed | PlanStepState::Cancelled
            )
        }) && self.stages.iter().all(|stage| {
            self.steps
                .iter()
                .any(|step| step.definition.stage_id == stage.id)
        }) {
            forecast.completed_at = self
                .steps
                .iter()
                .filter_map(|step| step.completed_at)
                .max()
                .unwrap_or(self.updated_at);
        }
        forecast
    }
}

/// Model-facing mutations never accept elapsed times or completion timestamps.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlanAction {
    Create {
        objective: String,
        acceptance: String,
        stages: Vec<PlanStage>,
        steps: Vec<PlanStepDefinition>,
    },
    Revise {
        stages: Vec<PlanStage>,
        steps: Vec<PlanStepDefinition>,
        reason: String,
    },
    Start {
        #[serde(rename = "stepId")]
        step_id: String,
    },
    Pause {
        #[serde(rename = "stepId")]
        step_id: String,
        reason: String,
    },
    Submit {
        #[serde(rename = "stepId")]
        step_id: String,
        evidence: Vec<String>,
    },
    Cancel {
        #[serde(rename = "stepId")]
        step_id: String,
        reason: String,
    },
    Read {
        #[serde(default)]
        offset: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanActionRequest {
    pub version: i64,
    pub operation: PlanAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum PlanReviewDecision {
    Continue,
    Accept,
    Revise,
    Interrupt,
    Reassign,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanAssessment {
    pub decision: PlanReviewDecision,
    pub step_id: String,
    pub reason: String,
    pub next_check_seconds: i64,
}
