use codex_protocol::continuous_planning::PlanStage;
use codex_protocol::continuous_planning::PlanStepDefinition;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(
    deny_unknown_fields,
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum SupervisorAction {
    Read {
        #[serde(default)]
        offset: usize,
    },
    Evidence {
        #[serde(default)]
        offset: usize,
    },
    Inspect {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        offset: usize,
    },
    Remember {
        text: String,
    },
    Create {
        version: i64,
        objective: String,
        acceptance: String,
        stages: Vec<PlanStage>,
        steps: Vec<PlanStepDefinition>,
    },
    Revise {
        version: i64,
        stages: Vec<PlanStage>,
        steps: Vec<PlanStepDefinition>,
        reason: String,
    },
    Select {
        version: i64,
        #[serde(rename = "stepId")]
        step_id: String,
    },
    Pause {
        reason: String,
    },
    Resume {},
    Replace {},
    Budget {
        tokens: Option<i64>,
    },
    Accept {
        version: i64,
        #[serde(rename = "stepId")]
        step_id: String,
        evidence: Vec<String>,
    },
    Cancel {
        version: i64,
        #[serde(rename = "stepId")]
        step_id: String,
        reason: String,
    },
}
