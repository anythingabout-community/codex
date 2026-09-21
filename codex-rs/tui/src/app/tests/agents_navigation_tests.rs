use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn agents_navigation_requires_local_daemon() -> Result<()> {
    let (mut app, mut events, _op_rx) = make_test_app_with_channels().await;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    let mut app_server = start_config_write_test_app_server(&app).await?;
    let endpoint = crate::RemoteAppServerEndpoint::UnixSocket {
        socket_path: AbsolutePathBuf::relative_to_current_dir("codex.sock")?,
    };
    for target in [
        AppServerTarget::Embedded,
        AppServerTarget::Remote {
            endpoint: endpoint.clone(),
        },
        AppServerTarget::LocalDaemon { endpoint },
    ] {
        let enabled = matches!(target, AppServerTarget::LocalDaemon { .. });
        app.app_server_target = target;
        let init = app.chatwidget_init_for_forked_or_resumed_thread(
            &mut tui,
            app.config.clone(),
            /*initial_user_message*/ None,
        );
        app.replace_chat_widget(ChatWidget::new_with_app_event(init));
        while events.try_recv().is_ok() {}
        app.handle_tui_event(
            &mut tui,
            &mut app_server,
            TuiEvent::Key(KeyCode::Left.into()),
        )
        .await?;
        if enabled {
            let event = events.try_recv()?;
            assert_matches!(event, AppEvent::OpenAgentsOverview);
            app.handle_event(&mut tui, &mut app_server, event).await?;
            assert!(!app.chat_widget.no_modal_or_popup_active());
        } else {
            assert!(events.try_recv().is_err());
            assert!(app.chat_widget.no_modal_or_popup_active());
            assert!(!render_bottom_popup(&app.chat_widget, /*width*/ 96).contains("for agents"));
        }
    }
    app_server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn shift_tab_cycles_supervised_conversations_and_restores_draft() -> Result<()> {
    let (mut app, _events, _op_rx) = make_test_app_with_channels().await;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    let mut server = start_config_write_test_app_server(&app).await?;
    let root = ThreadId::new();
    let child = ThreadId::new();
    app.config.features.enable(Feature::ContinuousPlanning)?;
    app.primary_thread_id = Some(root);
    app.primary_session_configured = Some(test_thread_session(root, app.config.cwd.to_path_buf()));
    for (id, role) in [(root, "supervisor"), (child, "implementer")] {
        let mut channel = ThreadEventChannel::new_with_session(
            THREAD_EVENT_CHANNEL_CAPACITY,
            test_thread_session(id, app.config.cwd.to_path_buf()),
            Vec::new(),
        );
        channel.mark_replay_only();
        app.thread_event_channels.insert(id, channel);
        app.upsert_agent_picker_thread(
            id,
            /*agent_nickname*/ None,
            Some(role.into()),
            /*is_closed*/ true,
        );
    }
    app.select_agent_thread(&mut tui, &mut server, root).await?;
    app.chat_widget
        .apply_external_edit("keep my Supervisor draft".into());
    // Legacy terminals emit BackTab, enhanced keyboards can emit Shift+Tab.
    for (key, expected) in [
        (KeyEvent::from(KeyCode::BackTab), child),
        (KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT), root),
        (KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT), child),
        (KeyEvent::from(KeyCode::BackTab), root),
    ] {
        app.handle_key_event(&mut tui, &mut server, key).await;
        assert_eq!(app.current_displayed_thread_id(), Some(expected));
        if expected == root {
            assert_eq!(
                app.chat_widget.composer_text_with_pending(),
                "keep my Supervisor draft"
            );
        } else {
            assert!(app.agent_navigation.is_parent_owned(child));
            assert!(app.chat_widget.composer_text_with_pending().is_empty());
        }
    }
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn background_supervisor_discovers_implementer_without_reopening_closed_conversation()
-> Result<()> {
    let (mut app, _events, _op_rx) = make_test_app_with_channels().await;
    app.config.features.enable(Feature::ContinuousPlanning)?;
    let supervisor = ThreadId::new();
    let implementer = ThreadId::new();
    let update = ServerNotification::ThreadSupervisorUpdated(
        codex_app_server_protocol::ThreadSupervisorUpdatedNotification {
            thread_id: supervisor.to_string(),
            observed_at: 1,
            state: codex_protocol::supervisor::SupervisorState {
                implementer_thread_id: Some(implementer.to_string()),
                ..Default::default()
            },
            plan: None,
        },
    );
    app.enqueue_thread_notification(supervisor, update.clone())
        .await?;
    assert_eq!(
        app.agent_navigation
            .get(&supervisor)
            .unwrap()
            .agent_role
            .as_deref(),
        Some("supervisor")
    );
    assert_eq!(
        app.agent_navigation
            .get(&implementer)
            .unwrap()
            .agent_role
            .as_deref(),
        Some("implementer")
    );
    assert!(app.agent_navigation.is_parent_owned(implementer));
    app.agent_navigation.mark_closed(implementer);
    app.enqueue_thread_notification(supervisor, update).await?;
    assert!(app.agent_navigation.get(&implementer).unwrap().is_closed);
    Ok(())
}
