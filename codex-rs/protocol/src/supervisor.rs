//! Durable ownership of the single Implementer in a Supervisor conversation.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct SupervisorState {
    #[serde(default)]
    #[ts(type = "number")]
    pub revision: i64,
    pub implementer_thread_id: Option<String>,
    pub execution_id: Option<String>,
    pub step_id: Option<String>,
    pub paused: bool,
    #[ts(type = "number")]
    pub updated_at: i64,
    #[ts(type = "number")]
    pub activity_sequence: i64,
    #[ts(type = "number")]
    pub total_tokens: i64,
    #[ts(type = "number | null")]
    pub token_budget: Option<i64>,
}
