use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use codex_app_server_protocol::*;
use codex_features::Feature;
use codex_protocol::continuous_planning::PlanStepState;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::Duration;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

pub(super) fn operation(id: &str, name: &str, args: Value) -> String {
    responses::sse(vec![
        responses::ev_response_created(id),
        responses::ev_function_call(id, name, &args.to_string()),
        responses::ev_completed_with_tokens(id, /*total_tokens*/ 5),
    ])
}

pub(super) fn message(id: &str, text: &str) -> String {
    responses::sse(vec![
        responses::ev_response_created(id),
        responses::ev_assistant_message(id, text),
        responses::ev_completed(id),
    ])
}

// Supervisor text uses batches; Implementer fixtures remain ordinary final replies.
fn supervisor_response(response: &str) -> String {
    response
        .lines()
        .map(|line| {
            let Some(data) = line.strip_prefix("data: ") else {
                return line.to_string();
            };
            let mut event: Value = serde_json::from_str(data).expect("SSE JSON");
            if event["item"]["type"] == "message"
                && let Some(parts) = event["item"]["content"].as_array_mut()
            {
                for part in parts {
                    if let Some(text) = part["text"].as_str()
                        && !text.starts_with("*** Begin Messages")
                    {
                        part["text"] = json!(format!(
                            "*** Begin Messages\n*** Message To: User\n+{text}\n*** End Messages"
                        ));
                    }
                }
            }
            format!("data: {event}")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n\n"
}

pub(super) fn create() -> String {
    operation(
        "create",
        "continuous_planning",
        json!({"action":"create","version":0,"objective":"Verify a staged task","acceptance":"Command evidence verified","stages":[{"id":"s1","title":"Explore","acceptance":"Capture results","assumptions":[],"estimateSeconds":60}],"steps":[{"id":"one","stageId":"s1","title":"Inspect","acceptance":"Capture results","dependencies":[],"estimateSeconds":60}]}),
    )
}

pub(super) async fn server(
    supervisor: Vec<String>,
    main: Vec<String>,
) -> (MockServer, Arc<Mutex<VecDeque<String>>>) {
    let server = responses::start_mock_server().await;
    let supervisor = Arc::new(Mutex::new(VecDeque::from(supervisor)));
    let pending = supervisor.clone();
    let main = Mutex::new(VecDeque::from(main));
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).expect("request JSON");
            let queue: &Mutex<VecDeque<String>> = if body["tools"]
                .to_string()
                .contains("\"continuous_planning\"")
            {
                &supervisor
            } else {
                &main
            };
            let response = queue
                .lock()
                .expect("response queue")
                .pop_front()
                .unwrap_or_else(|| message("idle", "Waiting for instructions."));
            // Exercise the same argument contract the model sees, including enum field names.
            for line in response
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
            {
                let event: Value = serde_json::from_str(line).expect("SSE JSON");
                let item = &event["item"];
                if item["type"] == "function_call" && item["name"] == "continuous_planning" {
                    let arguments: Value = serde_json::from_str(
                        item["arguments"]
                            .as_str()
                            .expect("serialized tool arguments"),
                    )
                    .expect("tool arguments JSON");
                    let tool = body["tools"]
                        .as_array()
                        .expect("advertised tools")
                        .iter()
                        .find(|tool| tool["name"] == "continuous_planning")
                        .expect("Supervisor tool");
                    let schema = tool["parameters"]["oneOf"]
                        .as_array()
                        .expect("action schemas")
                        .iter()
                        .find(|schema| {
                            schema["properties"]["action"]["enum"][0] == arguments["action"]
                        })
                        .expect("matching action schema");
                    for required in schema["required"].as_array().expect("required fields") {
                        assert!(
                            arguments
                                .get(required.as_str().expect("required field name"))
                                .is_some(),
                            "model arguments must satisfy the advertised schema: {schema}"
                        );
                    }
                }
            }
            responses::sse_response(
                if body["tools"]
                    .to_string()
                    .contains("\"continuous_planning\"")
                {
                    supervisor_response(&response)
                } else {
                    response
                },
            )
        })
        .mount(&server)
        .await;
    (server, pending)
}

pub(super) async fn start(app: &mut TestAppServer, prompt: &str) -> Result<Thread> {
    let request = app
        .send_thread_start_request_with_auto_env(ThreadStartParams::default())
        .await?;
    let ThreadStartResponse { thread, .. } = app.read_response(request).await?;
    let request = app.send_raw_request("turn/start", Some(json!({"threadId":thread.id,"input":[{"type":"text","text":prompt,"textElements":[]}]}))).await?;
    let _: TurnStartResponse = app.read_response(request).await?;
    Ok(thread)
}

pub(super) async fn wait_for_text(
    app: &mut TestAppServer,
    thread_id: &str,
    text: &str,
) -> Result<()> {
    timeout(Duration::from_secs(45), async {
        let turn_id = loop {
            let done: ItemCompletedNotification = app.read_notification("item/completed").await?;
            if done.thread_id == thread_id && done.item.into_visible_messages().iter().any(|item| matches!(item, ThreadItem::AgentMessage { text: value, .. } if value.trim_end() == text)) { break done.turn_id; }
        };
        loop {
            let done: TurnCompletedNotification = app.read_notification("turn/completed").await?;
            if done.thread_id == thread_id && done.turn.id == turn_id { return Ok(()); }
        }
    }).await?
}

#[tokio::test]
async fn supervisor_owns_tools_and_accepts_recorded_implementer_evidence() -> Result<()> {
    let (server, _) = server(vec![
        operation("create", "continuous_planning", json!({"action":"create","version":0,"objective":"Verify a staged task","acceptance":"Command evidence verified","stages":[{"id":"S07","title":"Explore","acceptance":"Capture results","assumptions":[],"estimateSeconds":60}],"steps":[{"id":"A42","stageId":"S07","title":"Inspect","acceptance":"Capture results","dependencies":[],"estimateSeconds":60}]})),
        operation("run", "continuous_planning", json!({"action":"select","version":1,"stepId":"A42"})),
        message("run-dispatch", "*** Begin Messages\n*** Message To: Implementer\n+Run echo supervisor-native-evidence and report its actual output.\n*** Message To: User\n+I am checking the result.\n*** End Messages"),
        operation("unchecked-accept", "continuous_planning", json!({"action":"accept","version":3,"stepId":"A42","evidence":["Trust the report."]})),
        operation("evidence", "continuous_planning", json!({"action":"evidence"})),
        operation("accept", "continuous_planning", json!({"action":"accept","version":3,"stepId":"A42","evidence":["Verified the recorded command output."]})),
        message("done", "Verified."),
    ], vec![
        operation("command", "exec_command", json!({"cmd":"echo supervisor-native-evidence","yield_time_ms":1000})),
        message("main-done", "Private execution report."),
    ]).await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .with_sandbox_mode("danger-full-access")
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(&mut app, "Implement the staged task").await?;
    wait_for_text(&mut app, &thread.id, "Verified.").await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let read: ThreadSupervisorReadResponse = app.read_response(request).await?;
    let plan = read.plan.expect("durable plan");
    assert_eq!(plan.steps[0].state, PlanStepState::Completed);
    assert_eq!(
        plan.steps[0].definition,
        codex_protocol::continuous_planning::PlanStepDefinition {
            id: "A42".into(),
            stage_id: "S07".into(),
            title: "Inspect".into(),
            acceptance: "Capture results".into(),
            dependencies: vec![],
            estimate_seconds: 60,
        }
    );
    assert!(plan.steps[0].completed_at.is_some());
    assert_eq!(plan.steps[0].initial_estimate_seconds, 60);
    let state = read.state.expect("durable ownership");
    assert_eq!(state.total_tokens, 30);
    let main_id = state.implementer_thread_id.expect("single Implementer");
    assert_ne!(main_id, thread.id);
    let request = app
        .send_thread_read_request(ThreadReadParams {
            thread_id: main_id.clone(),
            include_turns: true,
        })
        .await?;
    let implementer: ThreadReadResponse = app.read_response(request).await?;
    assert_eq!(implementer.thread.can_accept_direct_input, Some(false));
    assert!(implementer.thread.turns.iter().flat_map(|turn| &turn.items).any(|item| matches!(item, ThreadItem::CommandExecution { aggregated_output: Some(output), .. } if output.contains("supervisor-native-evidence"))));
    let request = app
        .send_raw_request(
            "thread/supervisor/history/list",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let history: ThreadSupervisorHistoryListResponse = app.read_response(request).await?;
    assert!(history.data.iter().any(|event| matches!(&event.activity, SupervisorActivity::ItemCompleted(item) if matches!(&item.item, ThreadItem::CommandExecution { aggregated_output: Some(output), .. } if output.contains("supervisor-native-evidence")))));
    assert!(
        history
            .data
            .windows(2)
            .all(|events| events[0].sequence < events[1].sequence)
    );
    assert!(
        history
            .data
            .iter()
            .all(|event| event.thread_id == thread.id && event.implementer_thread_id == main_id)
    );
    let requests = server.received_requests().await.expect("captured requests");
    let bodies: Vec<Value> = requests
        .iter()
        .map(|request| serde_json::from_slice(&request.body).expect("request JSON"))
        .collect();
    let root = bodies
        .iter()
        .find(|body| {
            body["tools"]
                .to_string()
                .contains("\"continuous_planning\"")
        })
        .expect("Supervisor request");
    let tool_names: Vec<&str> = root["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(tool_names.contains(&"continuous_planning"));
    assert_eq!(tool_names.len(), root["tools"].as_array().unwrap().len());
    assert!(tool_names.iter().all(|name| matches!(
        *name,
        "continuous_planning" | "request_user_input" | "request_user_input_async"
    )));
    let main = bodies
        .iter()
        .find(|body| body["tools"].to_string().contains("exec_command"))
        .expect("Implementer request");
    assert!(
        main["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| !matches!(tool["name"].as_str(), Some("create_goal" | "update_plan")))
    );
    assert!(
        main["input"]
            .to_string()
            .contains("Implement the staged task")
    );
    assert!(bodies.iter().any(|body| {
        body["input"]
            .to_string()
            .contains("supervisor-native-evidence")
            && body["input"].to_string().contains("function_call_output")
    }));
    assert!(bodies.iter().any(|body| {
        body["input"]
            .to_string()
            .contains("read the submitted execution evidence before acceptance")
    }));
    assert!(bodies.iter().any(|body| {
        body["tools"]
            .to_string()
            .contains("\"continuous_planning\"")
            && body["input"]
                .to_string()
                .contains("Private execution report.")
    }));
    let request = app.send_raw_request("turn/start", Some(json!({"threadId":main_id,"input":[{"type":"text","text":"bypass Supervisor","textElements":[]}]}))).await?;
    let error = app
        .read_stream_until_error_message(RequestId::Integer(request))
        .await?;
    assert!(error.error.message.contains("direct"));
    app.shutdown_gracefully().await?;
    let mut restored = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let request = restored
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            ..Default::default()
        })
        .await?;
    let _: ThreadResumeResponse = restored.read_response(request).await?;
    let request = restored
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let resumed: ThreadSupervisorReadResponse = restored.read_response(request).await?;
    let restored_plan = resumed.plan.expect("restored plan");
    assert_eq!(
        (restored_plan.id, restored_plan.stages, restored_plan.steps),
        (plan.id, plan.stages, plan.steps)
    );
    let state = resumed.state.expect("restored ownership");
    assert_eq!(
        (
            state.paused,
            state.total_tokens,
            state.implementer_thread_id
        ),
        (true, 30, Some(main_id))
    );
    let request = restored
        .send_raw_request(
            "thread/supervisor/history/list",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let resumed_history: ThreadSupervisorHistoryListResponse =
        restored.read_response(request).await?;
    assert_eq!(resumed_history, history);
    Ok(())
}

#[tokio::test]
async fn supervisor_budget_interrupts_automatic_work_and_survives_readback() -> Result<()> {
    let (server, _) = server(
        vec![
            operation(
                "budget",
                "continuous_planning",
                json!({"action":"budget","tokens":10}),
            ),
            create(),
            message("unexpected", "The budget should stop this turn."),
        ],
        Vec::new(),
    )
    .await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .with_sandbox_mode("danger-full-access")
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(
        &mut app,
        "Implement the staged task with a total budget of 10 tokens.",
    )
    .await?;
    let done: TurnCompletedNotification = timeout(
        Duration::from_secs(30),
        app.read_notification("turn/completed"),
    )
    .await??;
    assert_eq!(done.turn.status, TurnStatus::Interrupted);
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let read: ThreadSupervisorReadResponse = app.read_response(request).await?;
    let state = read.state.expect("durable ownership");
    assert_eq!(
        (
            state.paused,
            state.total_tokens,
            state.token_budget,
            state.implementer_thread_id
        ),
        (true, 10, Some(10), None)
    );
    Ok(())
}

#[tokio::test]
async fn supervisor_pauses_execution_and_replaces_only_the_implementer_context() -> Result<()> {
    let (server, pending) = server(vec![
        create(),
        operation("run", "continuous_planning", json!({"action":"select","version":1,"stepId":"one"})),
        message("run-dispatch", "*** Begin Messages\n*** Message To: Implementer\n+Wait for the prerequisite.\n*** Message To: User\n+Execution started.\n*** End Messages"),
    ], vec![
        operation("wait-one", "exec_command", json!({"cmd":if cfg!(windows) { "Start-Sleep -Seconds 30" } else { "sleep 30" },"yield_time_ms":1000})),
        message("first-idle", "Still waiting."),
        operation("wait-two", "exec_command", json!({"cmd":if cfg!(windows) { "Start-Sleep -Seconds 30" } else { "sleep 30" },"yield_time_ms":1000})),
        message("second-idle", "Still waiting."),
    ]).await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .with_sandbox_mode("danger-full-access")
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(&mut app, "Implement the staged task").await?;
    wait_for_text(&mut app, &thread.id, "Execution started.").await?;
    let _: ThreadSupervisorActivityNotification = timeout(
        Duration::from_secs(30),
        app.read_notification("thread/supervisor/activity"),
    )
    .await??;
    let request = app
        .send_raw_request(
            "thread/supervisor/interrupt",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let _: ThreadSupervisorInterruptResponse = app.read_response(request).await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let paused: ThreadSupervisorReadResponse = app.read_response(request).await?;
    assert!(paused.state.as_ref().expect("state").paused);
    let plan = paused.plan.as_ref().expect("plan");
    assert_eq!(
        (plan.steps[0].state, plan.steps[0].running_since),
        (PlanStepState::Blocked, None)
    );
    pending.lock().expect("script").extend([
        operation("replace", "continuous_planning", json!({"action":"replace"})),
        operation("resume", "continuous_planning", json!({"action":"resume"})),
        message("replacement-dispatch", "*** Begin Messages\n*** Message To: Implementer\n+The prerequisite remains unavailable. Continue waiting for the prerequisite.\n*** Message To: User\n+Execution context refreshed.\n*** End Messages"),
    ]);
    let request = app.send_raw_request("turn/start", Some(json!({"threadId":thread.id,"input":[{"type":"text","text":"Refresh the execution context and continue.","textElements":[]}]}))).await?;
    let _: TurnStartResponse = app.read_response(request).await?;
    wait_for_text(&mut app, &thread.id, "Execution context refreshed.").await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let resumed: ThreadSupervisorReadResponse = app.read_response(request).await?;
    assert_ne!(
        paused.state.unwrap().implementer_thread_id,
        resumed.state.unwrap().implementer_thread_id
    );
    assert_eq!(resumed.plan.as_ref().unwrap().id, plan.id);
    assert_eq!(
        resumed.plan.as_ref().unwrap().steps[0].initial_estimate_seconds,
        60
    );
    timeout(Duration::from_secs(30), async {
        loop {
            if server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|request| {
                    String::from_utf8_lossy(&request.body)
                        .contains("The prerequisite remains unavailable")
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/interrupt",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let _: ThreadSupervisorInterruptResponse = app.read_response(request).await?;
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().any(|request| {
        String::from_utf8_lossy(&request.body).contains("The prerequisite remains unavailable")
    }));
    Ok(())
}

#[tokio::test]
async fn supervisor_routes_implementer_approval_and_resolution_to_the_user_conversation()
-> Result<()> {
    let (server, _) = server(vec![
        create(),
        operation("run", "continuous_planning", json!({"action":"select","version":1,"stepId":"one"})),
        message("run-dispatch", "*** Begin Messages\n*** Message To: Implementer\n+Run the authorized command with the required approval.\n*** Message To: User\n+I will check the command result.\n*** End Messages"),
        operation("evidence", "continuous_planning", json!({"action":"evidence"})),
        operation("accept", "continuous_planning", json!({"action":"accept","version":3,"stepId":"one","evidence":["The approved command returned the expected output."]})),
        message("done", "Approved command verified."),
    ], vec![
        operation("approved-command", "exec_command", json!({"cmd":"echo approved-supervisor-command","sandbox_permissions":"require_escalated","justification":"Approve the requested test command?","yield_time_ms":1000})),
        message("main-done", "Private execution report."),
    ]).await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .with_approval_policy("on-request")
        .with_sandbox_mode("read-only")
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(
        &mut app,
        "Run and verify the test command; ask me for its required approval.",
    )
    .await?;
    let request = timeout(
        Duration::from_secs(30),
        app.read_stream_until_request_message(),
    )
    .await??;
    let expected_request = request.clone();
    let ServerRequest::CommandExecutionRequestApproval { request_id, params } = request else {
        anyhow::bail!("expected command approval")
    };
    assert_eq!(params.thread_id, thread.id);
    let resume = app
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            ..Default::default()
        })
        .await?;
    let _: ThreadResumeResponse = app.read_response(resume).await?;
    let replayed = timeout(
        Duration::from_secs(30),
        app.read_stream_until_request_message(),
    )
    .await??;
    assert_eq!(replayed, expected_request);
    let expected_request_id = request_id.clone();
    app.send_response(
        request_id,
        serde_json::to_value(CommandExecutionRequestApprovalResponse {
            decision: CommandExecutionApprovalDecision::Accept,
        })?,
    )
    .await?;
    let resolved: ServerRequestResolvedNotification = timeout(
        Duration::from_secs(30),
        app.read_notification("serverRequest/resolved"),
    )
    .await??;
    assert_eq!(
        (resolved.thread_id, resolved.request_id),
        (thread.id.clone(), expected_request_id)
    );
    wait_for_text(&mut app, &thread.id, "Approved command verified.").await?;
    Ok(())
}

#[tokio::test]
async fn delegated_subagent_owns_a_supervisor_and_read_only_implementer() -> Result<()> {
    let server = responses::start_mock_server().await;
    let root = Mutex::new(VecDeque::from(vec![
        create(),
        operation(
            "root-run",
            "continuous_planning",
            json!({"action":"select","version":1,"stepId":"one"}),
        ),
        message(
            "root-run-dispatch",
            "*** Begin Messages\n*** Message To: Implementer\n+delegate-probe-root: delegate the inspection to a subagent.\n*** End Messages",
        ),
    ]));
    let child = Mutex::new(VecDeque::from(vec![
        create(),
        operation(
            "child-run",
            "continuous_planning",
            json!({"action":"select","version":1,"stepId":"one"}),
        ),
        message(
            "child-run-dispatch",
            "*** Begin Messages\n*** Message To: Implementer\n+Complete the child inspection.\n*** End Messages",
        ),
    ]));
    let implementer = Mutex::new(VecDeque::from(vec![responses::sse(vec![responses::ev_response_created("delegate"), responses::ev_function_call_with_namespace("delegate", "collaboration", "spawn_agent", &json!({"message":"child-supervision-probe: inspect this task through your Implementer.","task_name":"inspection","fork_turns":"none"}).to_string()), responses::ev_completed("delegate")])]));
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).expect("request JSON");
            let input = body["input"].to_string();
            let supervising = body["tools"]
                .to_string()
                .contains("\"continuous_planning\"");
            let response = if supervising {
                let queue = if input.contains("child-supervision-probe") {
                    &child
                } else {
                    &root
                };
                queue
                    .lock()
                    .expect("queue")
                    .pop_front()
                    .unwrap_or_else(|| message("waiting", "Waiting for evidence."))
            } else if input.contains("delegate-probe-root") {
                implementer
                    .lock()
                    .expect("queue")
                    .pop_front()
                    .unwrap_or_else(|| message("delegated", "Waiting for the child Supervisor."))
            } else {
                message("implemented", "child-implementation-complete")
            };
            responses::sse_response(
                if body["tools"]
                    .to_string()
                    .contains("\"continuous_planning\"")
                {
                    supervisor_response(&response)
                } else {
                    response
                },
            )
        })
        .mount(&server)
        .await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .with_model("gpt-5.6-sol")
        .enable_feature(Feature::ContinuousPlanning)
        .enable_feature(Feature::Collab)
        .enable_feature(Feature::MultiAgentV2)
        .write(home.path())?;
    app_test_support::write_models_cache(home.path()).await?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let root = start(&mut app, "Inspect the task with delegation.").await?;
    // The nested Implementer's lifecycle must arrive as native conversation events.
    let implemented = timeout(Duration::from_secs(45), async {
        loop {
            let done: ItemCompletedNotification = app.read_notification("item/completed").await?;
            if done.item.clone().into_visible_messages().iter().any(|item| matches!(item, ThreadItem::AgentMessage { text, .. } if text == "child-implementation-complete")) {
                return Ok::<_, anyhow::Error>(done);
            }
        }
    }).await??;
    let request = app
        .send_thread_read_request(ThreadReadParams {
            thread_id: implemented.thread_id.clone(),
            include_turns: true,
        })
        .await?;
    let implementation: ThreadReadResponse = app.read_response(request).await?;
    assert_eq!(implementation.thread.can_accept_direct_input, Some(false));
    let SessionSource::SubAgent(codex_protocol::protocol::SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth,
        agent_path,
        ..
    }) = implementation.thread.source
    else {
        panic!("Implementer must be a child conversation");
    };
    assert_eq!(
        (
            depth,
            agent_path.as_ref().map(codex_protocol::AgentPath::as_str)
        ),
        (3, Some("/root/implementer/inspection/implementer"))
    );
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":parent_thread_id})),
        )
        .await?;
    let child: ThreadSupervisorReadResponse = app.read_response(request).await?;
    assert_eq!(
        child
            .state
            .expect("child Supervisor runtime")
            .implementer_thread_id,
        Some(implemented.thread_id.clone())
    );
    assert_ne!(parent_thread_id.to_string(), root.id);
    let request = app
        .send_raw_request(
            "thread/supervisor/interrupt",
            Some(json!({"threadId":root.id})),
        )
        .await?;
    let _: ThreadSupervisorInterruptResponse = app.read_response(request).await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":parent_thread_id})),
        )
        .await?;
    let paused: ThreadSupervisorReadResponse = app.read_response(request).await?;
    assert!(paused.state.expect("child state").paused);
    assert_eq!(paused.plan.expect("child plan").reason, "Execution paused");
    let request = app.send_raw_request("turn/start", Some(json!({"threadId":implemented.thread_id,"input":[{"type":"text","text":"bypass child Supervisor","textElements":[]}]}))).await?;
    let error = app
        .read_stream_until_error_message(RequestId::Integer(request))
        .await?;
    assert!(error.error.message.contains("direct"));
    app.shutdown_gracefully().await?;
    Ok(())
}
