//! Turn item transformation is the admission gate, before any client display.

use crate::message_parser::MessageParser;
use crate::runtime::ImplementerBinding;
use crate::runtime::PlanRuntime;
use codex_extension_api::*;
use codex_extension_items::ExtensionItem;
use codex_extension_items::continuous_planning::DirectedMessage;
use codex_extension_items::continuous_planning::MessageBatch;
use codex_extension_items::continuous_planning::MessageRecipient;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::TurnItem;
use codex_protocol::models::MessagePhase;
use std::sync::atomic::Ordering;

pub(crate) struct MessageItems;
#[derive(Clone)]
pub(crate) struct ImplementerReply {
    pub id: String,
    pub text: String,
}

impl TurnItemContributor for MessageItems {
    fn enabled_for(&self, thread_store: &ExtensionData) -> bool {
        thread_store.get::<PlanRuntime>().is_some()
            || thread_store.get::<ImplementerBinding>().is_some()
    }

    fn message_validator(
        &self,
        thread_store: &ExtensionData,
    ) -> Option<Box<dyn MessageStreamValidator>> {
        thread_store
            .get::<PlanRuntime>()
            .map(|_| Box::new(MessageParser::default()) as Box<dyn MessageStreamValidator>)
    }

    fn defer_streaming(&self, _thread_store: &ExtensionData) -> bool {
        false
    }

    fn contribute<'a>(
        &'a self,
        thread_store: &'a ExtensionData,
        turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let TurnItem::AgentMessage(message) = item else {
                return Ok(());
            };
            let text = message
                .content
                .iter()
                .map(|part| match part {
                    AgentMessageContent::Text { text } => text.as_str(),
                })
                .collect::<String>();
            let final_answer = !matches!(message.phase, Some(MessagePhase::Commentary));
            if thread_store.get::<ImplementerBinding>().is_some() {
                if final_answer && !text.trim().is_empty() {
                    let end = text.floor_char_boundary(text.len().min(7800));
                    let reply = if end < text.len() {
                        format!(
                            "{}\n[Reply truncated; read the source Implementer turn for the full report.]",
                            &text[..end]
                        )
                    } else {
                        text.clone()
                    };
                    turn_store.insert(ImplementerReply {
                        id: message.id.clone(),
                        text: reply,
                    });
                }
                *item =
                    TurnItem::Extension(ExtensionItem::ContinuousPlanningMessages(MessageBatch {
                        id: message.id.clone(),
                        sender: MessageRecipient::Implementer,
                        audience: MessageRecipient::Implementer,
                        messages: vec![DirectedMessage {
                            id: format!("{}:1", message.id),
                            recipient: MessageRecipient::Supervisor,
                            text,
                        }],
                        final_answer,
                    }));
                return Ok(());
            }
            let Some(runtime) = thread_store.get::<PlanRuntime>() else {
                return Ok(());
            };
            let mut parser = MessageParser::default();
            let parsed = parser
                .push(&text)
                .and_then(|()| parser.finish(&message.id, final_answer));
            let result = match parsed {
                Ok(batch) => runtime
                    .dispatch(&batch)
                    .await
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error),
            };
            let batch = match result {
                Ok(batch) => batch,
                Err(error) => {
                    let messages = {
                        let mut state = runtime.messages.lock().await;
                        if state.correction_attempted {
                            runtime.active.store(false, Ordering::SeqCst);
                            state.feedback = None;
                            vec![DirectedMessage {
                                id: format!("{}:1", message.id),
                                recipient: MessageRecipient::User,
                                text: format!("Continuous Planning paused: {error}"),
                            }]
                        } else {
                            state.correction_attempted = true;
                            state.feedback = Some(format!(
                                "{error}. This batch was rejected before any new delivery. Previously accepted deliveries are preserved. Correct the complete document using <messages>, <user> or <implementer> blocks, and </messages>. You have one correction attempt."
                            ));
                            Vec::new()
                        }
                    };
                    let pause = !messages.is_empty();
                    if pause {
                        let _ = runtime.suspend().await;
                    }
                    MessageBatch {
                        id: message.id.clone(),
                        sender: MessageRecipient::Supervisor,
                        audience: MessageRecipient::User,
                        messages,
                        final_answer,
                    }
                }
            };
            *item = TurnItem::Extension(ExtensionItem::ContinuousPlanningMessages(batch));
            Ok(())
        })
    }
}
