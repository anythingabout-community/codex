//! Host-addressed messages. Rendering these items must never dispatch them again.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum MessageRecipient {
    User,
    Supervisor,
    Implementer,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct DirectedMessage {
    pub id: String,
    pub recipient: MessageRecipient,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MessageBatch {
    /// The source assistant message ID; block IDs append their one-based index.
    pub id: String,
    pub sender: MessageRecipient,
    /// Conversation in which this host-generated projection is presented.
    pub audience: MessageRecipient,
    pub messages: Vec<DirectedMessage>,
    pub final_answer: bool,
}
