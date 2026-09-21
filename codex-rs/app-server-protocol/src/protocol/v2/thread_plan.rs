use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

pub use codex_protocol::continuous_planning::ContinuousPlanning;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPlanReadParams {
    pub thread_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPlanReadResponse {
    pub plan: Option<ContinuousPlanning>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPlanHistoryListParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPlanHistoryListResponse {
    pub data: Vec<ContinuousPlanning>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadPlanUpdatedNotification {
    pub thread_id: String,
    /// Server clock when this view was sent, including snapshots on resume.
    #[ts(type = "number")]
    pub observed_at: i64,
    pub plan: ContinuousPlanning,
}
