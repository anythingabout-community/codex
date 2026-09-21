use crate::outgoing_message::OutgoingMessageSender;
use crate::thread_state::ThreadStateManager;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadSupervisorUpdatedNotification;
use codex_protocol::ThreadId;
use codex_rollout::state_db::StateDbHandle;
use codex_supervisor_extension::PlanUpdateSink;
use codex_supervisor_extension::SupervisorUpdateSink;
use std::sync::Arc;

pub(crate) fn plan_update_sink(
    outgoing: Arc<OutgoingMessageSender>,
    threads: ThreadStateManager,
    db: Option<StateDbHandle>,
) -> PlanUpdateSink {
    Arc::new(move |thread_id, plan| {
        let outgoing = outgoing.clone();
        let threads = threads.clone();
        let db = db.clone();
        tokio::spawn(async move {
            let Some(db) = db else {
                return;
            };
            let state = db
                .read_supervisor(thread_id)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let connections = threads.subscribed_connection_ids(thread_id).await;
            outgoing
                .send_server_notification_to_connections(
                    &connections,
                    ServerNotification::ThreadSupervisorUpdated(
                        ThreadSupervisorUpdatedNotification {
                            thread_id: thread_id.to_string(),
                            observed_at: chrono::Utc::now().timestamp(),
                            state,
                            plan: Some(plan),
                        },
                    ),
                )
                .await;
        });
    })
}

pub(crate) fn supervisor_update_sink(
    outgoing: Arc<OutgoingMessageSender>,
    threads: ThreadStateManager,
    db: Option<StateDbHandle>,
) -> SupervisorUpdateSink {
    Arc::new(move |thread_id, state| {
        let outgoing = outgoing.clone();
        let threads = threads.clone();
        let db = db.clone();
        tokio::spawn(async move {
            let Some(db) = db else {
                return;
            };
            let plan = db.read_thread_plan(thread_id).await.ok().flatten();
            let connections = threads.subscribed_connection_ids(thread_id).await;
            outgoing
                .send_server_notification_to_connections(
                    &connections,
                    ServerNotification::ThreadSupervisorUpdated(
                        ThreadSupervisorUpdatedNotification {
                            thread_id: thread_id.to_string(),
                            observed_at: chrono::Utc::now().timestamp(),
                            state,
                            plan,
                        },
                    ),
                )
                .await;
        });
    })
}

pub(crate) async fn send_plan_snapshot(
    outgoing: &OutgoingMessageSender,
    connection_id: crate::outgoing_message::ConnectionId,
    thread_id: ThreadId,
    db: &codex_state::StateRuntime,
) {
    if let Ok(Some(state)) = db.read_supervisor(thread_id).await {
        let plan = db.read_thread_plan(thread_id).await.ok().flatten();
        outgoing
            .send_server_notification_to_connections(
                &[connection_id],
                ServerNotification::ThreadSupervisorUpdated(ThreadSupervisorUpdatedNotification {
                    thread_id: thread_id.to_string(),
                    observed_at: chrono::Utc::now().timestamp(),
                    state,
                    plan,
                }),
            )
            .await;
    }
}
