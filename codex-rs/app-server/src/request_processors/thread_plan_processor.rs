use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::ThreadPlanHistoryListParams;
use codex_app_server_protocol::ThreadPlanHistoryListResponse;
use codex_app_server_protocol::ThreadPlanReadParams;
use codex_app_server_protocol::ThreadPlanReadResponse;
use codex_protocol::ThreadId;
use codex_rollout::state_db::StateDbHandle;

use crate::error_code::internal_error;
use crate::error_code::invalid_request;

pub(crate) struct ThreadPlanRequestProcessor(
    pub Option<StateDbHandle>,
    pub std::sync::Arc<codex_core::ThreadManager>,
);

impl ThreadPlanRequestProcessor {
    pub async fn read(
        &self,
        params: ThreadPlanReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let id = ThreadId::try_from(params.thread_id)
            .map_err(|error| invalid_request(error.to_string()))?;
        let db = self
            .0
            .as_ref()
            .ok_or_else(|| internal_error("plan storage unavailable"))?;
        let plan = db
            .read_thread_plan(id)
            .await
            .map_err(|error| internal_error(error.to_string()))?;
        Ok(Some(ThreadPlanReadResponse { plan }.into()))
    }

    pub async fn history(
        &self,
        params: ThreadPlanHistoryListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let id = ThreadId::try_from(params.thread_id)
            .map_err(|error| invalid_request(error.to_string()))?;
        let before = params
            .cursor
            .map(|cursor| cursor.parse::<i64>())
            .transpose()
            .map_err(|error| invalid_request(error.to_string()))?
            .unwrap_or(i64::MAX);
        if before <= 0 {
            return Err(invalid_request("invalid plan cursor"));
        }
        let limit = params.limit.unwrap_or(20).clamp(1, 99);
        let db = self
            .0
            .as_ref()
            .ok_or_else(|| internal_error("plan storage unavailable"))?;
        let mut data = db
            .list_thread_plan_history(id, before, limit + 1)
            .await
            .map_err(|error| internal_error(error.to_string()))?;
        let more = data.len() > limit as usize;
        data.truncate(limit as usize);
        let next_cursor = data
            .last()
            .filter(|_| more)
            .map(|plan| plan.version.to_string());
        Ok(Some(
            ThreadPlanHistoryListResponse { data, next_cursor }.into(),
        ))
    }
}

impl ThreadPlanRequestProcessor {
    pub async fn supervisor_read(
        &self,
        params: codex_app_server_protocol::ThreadSupervisorReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let id =
            ThreadId::try_from(params.thread_id).map_err(|e| invalid_request(e.to_string()))?;
        let db = self
            .0
            .as_ref()
            .ok_or_else(|| internal_error("Supervisor storage unavailable"))?;
        let state = db
            .read_supervisor(id)
            .await
            .map_err(|e| internal_error(e.to_string()))?;
        let plan = db
            .read_thread_plan(id)
            .await
            .map_err(|e| internal_error(e.to_string()))?;
        Ok(Some(
            codex_app_server_protocol::ThreadSupervisorReadResponse { state, plan }.into(),
        ))
    }

    pub async fn supervisor_history(
        &self,
        params: codex_app_server_protocol::ThreadSupervisorHistoryListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let id =
            ThreadId::try_from(params.thread_id).map_err(|e| invalid_request(e.to_string()))?;
        let after = params
            .cursor
            .map(|s| s.parse::<i64>())
            .transpose()
            .map_err(|e| invalid_request(e.to_string()))?
            .unwrap_or(0);
        if after < 0 {
            return Err(invalid_request("invalid cursor"));
        }
        let limit = params.limit.unwrap_or(50).clamp(1, 99);
        let db = self
            .0
            .as_ref()
            .ok_or_else(|| internal_error("Supervisor storage unavailable"))?;
        let mut rows = db
            .list_supervisor_activity(id, after, limit + 1)
            .await
            .map_err(|e| internal_error(e.to_string()))?;
        let more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        let next_cursor = more
            .then(|| rows.last().map(|(sequence, _)| sequence.to_string()))
            .flatten();
        let data = rows
            .into_iter()
            .map(|(sequence, value)| {
                let mut event: codex_app_server_protocol::ThreadSupervisorActivityNotification =
                    serde_json::from_str(&value)?;
                event.sequence = sequence;
                Ok(event)
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()
            .map_err(|e| internal_error(e.to_string()))?;
        Ok(Some(
            codex_app_server_protocol::ThreadSupervisorHistoryListResponse { data, next_cursor }
                .into(),
        ))
    }

    pub async fn supervisor_interrupt(
        &self,
        params: codex_app_server_protocol::ThreadSupervisorInterruptParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let id =
            ThreadId::try_from(params.thread_id).map_err(|e| invalid_request(e.to_string()))?;
        let thread = self
            .1
            .get_thread(id)
            .await
            .map_err(|e| invalid_request(e.to_string()))?;
        if !codex_supervisor_extension::is_supervisor(&thread) {
            return Err(invalid_request("thread is not a Supervisor conversation"));
        }
        codex_supervisor_extension::suspend(&thread)
            .await
            .map_err(|e| internal_error(e.to_string()))?;
        thread
            .submit(codex_protocol::protocol::Op::Interrupt)
            .await
            .map_err(|e| internal_error(e.to_string()))?;
        Ok(Some(
            codex_app_server_protocol::ThreadSupervisorInterruptResponse {}.into(),
        ))
    }
}
