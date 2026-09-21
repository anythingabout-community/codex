use super::*;
use crate::JsonSchema;
use crate::TS;
use codex_protocol::continuous_planning::ContinuousPlanning;
use codex_protocol::supervisor::SupervisorState;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSupervisorReadParams {
    pub thread_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSupervisorReadResponse {
    pub state: Option<SupervisorState>,
    pub plan: Option<ContinuousPlanning>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSupervisorUpdatedNotification {
    pub thread_id: String,
    #[ts(type = "number")]
    pub observed_at: i64,
    pub state: SupervisorState,
    pub plan: Option<ContinuousPlanning>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSupervisorHistoryListParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSupervisorHistoryListResponse {
    #[schemars(with = "Vec<InlineNotification<ThreadSupervisorActivityNotification>>")]
    pub data: Vec<ThreadSupervisorActivityNotification>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSupervisorInterruptParams {
    pub thread_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSupervisorInterruptResponse {}

/// Only native tool activity crosses this boundary, never Implementer dialogue.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
#[ts(
    tag = "type",
    content = "data",
    rename_all = "camelCase",
    export_to = "v2/"
)]
pub enum SupervisorActivity {
    ItemStarted(
        #[schemars(with = "InlineNotification<ItemStartedNotification>")] ItemStartedNotification,
    ),
    ItemCompleted(
        #[schemars(with = "InlineNotification<ItemCompletedNotification>")]
        ItemCompletedNotification,
    ),
    CommandOutput(
        #[schemars(with = "InlineNotification<CommandExecutionOutputDeltaNotification>")]
        CommandExecutionOutputDeltaNotification,
    ),
    FileOutput(
        #[schemars(with = "InlineNotification<FileChangeOutputDeltaNotification>")]
        FileChangeOutputDeltaNotification,
    ),
    TerminalInteraction(
        #[schemars(with = "InlineNotification<TerminalInteractionNotification>")]
        TerminalInteractionNotification,
    ),
    McpProgress(
        #[schemars(with = "InlineNotification<McpToolCallProgressNotification>")]
        McpToolCallProgressNotification,
    ),
}

impl SupervisorActivity {
    pub fn from_notification(notification: crate::ServerNotification) -> Option<Self> {
        match notification {
            crate::ServerNotification::ItemStarted(value) if execution_item(&value.item) => {
                Some(Self::ItemStarted(value))
            }
            crate::ServerNotification::ItemCompleted(value) if execution_item(&value.item) => {
                Some(Self::ItemCompleted(value))
            }
            crate::ServerNotification::CommandExecutionOutputDelta(value) => {
                Some(Self::CommandOutput(value))
            }
            crate::ServerNotification::FileChangeOutputDelta(value) => {
                Some(Self::FileOutput(value))
            }
            crate::ServerNotification::TerminalInteraction(value) => {
                Some(Self::TerminalInteraction(value))
            }
            crate::ServerNotification::McpToolCallProgress(value) => Some(Self::McpProgress(value)),
            // This allowlist deliberately excludes messages, reasoning and lifecycle control.
            _ => None,
        }
    }
    pub fn into_notification(self) -> crate::ServerNotification {
        match self {
            Self::ItemStarted(value) => crate::ServerNotification::ItemStarted(value),
            Self::ItemCompleted(value) => crate::ServerNotification::ItemCompleted(value),
            Self::CommandOutput(value) => {
                crate::ServerNotification::CommandExecutionOutputDelta(value)
            }
            Self::FileOutput(value) => crate::ServerNotification::FileChangeOutputDelta(value),
            Self::TerminalInteraction(value) => {
                crate::ServerNotification::TerminalInteraction(value)
            }
            Self::McpProgress(value) => crate::ServerNotification::McpToolCallProgress(value),
        }
    }
}

fn execution_item(item: &ThreadItem) -> bool {
    matches!(
        item,
        ThreadItem::CommandExecution { .. }
            | ThreadItem::FileChange { .. }
            | ThreadItem::McpToolCall { .. }
            | ThreadItem::WebSearch { .. }
            | ThreadItem::ImageGeneration { .. }
    )
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadSupervisorActivityNotification {
    pub thread_id: String,
    pub implementer_thread_id: String,
    pub execution_id: String,
    pub step_id: String,
    pub title: String,
    #[ts(type = "number")]
    pub sequence: i64,
    #[ts(type = "number")]
    pub created_at: i64,
    pub activity: SupervisorActivity,
}

// Notification payloads are exported as titled roots. Inline their identical wire
// shape when nested in an execution record so the bundle does not also register
// an untitled definition under the same notification name.
#[cfg(test)]
struct InlineNotification<T>(std::marker::PhantomData<T>);

#[cfg(test)]
impl<T: JsonSchema> JsonSchema for InlineNotification<T> {
    fn is_referenceable() -> bool {
        false
    }
    fn schema_name() -> String {
        T::schema_name()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::schema::Schema {
        T::json_schema(generator)
    }
}
