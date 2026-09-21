//! Checkpoint execution evidence separately from the native Implementer conversation.

use super::ThreadScopedOutgoingMessageSender;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::SupervisorActivity;
use codex_app_server_protocol::ThreadSupervisorActivityNotification;

impl ThreadScopedOutgoingMessageSender {
    pub(super) async fn supervisor_notification(
        &self,
        notification: ServerNotification,
    ) -> Option<ServerNotification> {
        let Some(binding) = &self.supervisor else {
            return Some(notification);
        };
        if let ServerNotification::ServerRequestResolved(mut resolved) = notification {
            resolved.thread_id = binding.supervisor_thread_id()?.to_string();
            return Some(ServerNotification::ServerRequestResolved(resolved));
        }
        let activity = SupervisorActivity::from_notification(notification)?;
        let turn_id = match &activity {
            SupervisorActivity::ItemStarted(event) => &event.turn_id,
            SupervisorActivity::ItemCompleted(event) => &event.turn_id,
            SupervisorActivity::CommandOutput(event) => &event.turn_id,
            SupervisorActivity::FileOutput(event) => &event.turn_id,
            SupervisorActivity::TerminalInteraction(event) => &event.turn_id,
            SupervisorActivity::McpProgress(event) => &event.turn_id,
        };
        let (owner, execution_id, step_id, title) = binding.presentation(turn_id).await?;
        let mut event = ThreadSupervisorActivityNotification {
            thread_id: owner.to_string(),
            implementer_thread_id: self.thread_id.to_string(),
            execution_id,
            step_id,
            title,
            sequence: 0,
            created_at: chrono::Utc::now().timestamp(),
            activity,
        };
        let checkpoint = crate::notification_media::without_notification_media(
            ServerNotification::ThreadSupervisorActivity(event.clone()),
        );
        let ServerNotification::ThreadSupervisorActivity(checkpoint) = checkpoint else {
            return None;
        };
        let serialized = serde_json::to_string(&checkpoint).ok()?;
        event.sequence = match binding.record_activity(&serialized).await {
            Ok(sequence) => sequence,
            Err(error) => {
                tracing::warn!(%error, "Supervisor activity checkpoint failed");
                return None;
            }
        };
        Some(ServerNotification::ThreadSupervisorActivity(event))
    }
}
