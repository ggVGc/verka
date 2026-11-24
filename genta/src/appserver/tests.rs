use super::*;

fn sent(actions: &[Action]) -> Vec<Value> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::Send(line) => serde_json::from_str(line).ok(),
            _ => None,
        })
        .collect()
}

#[test]
fn start_sends_initialize() {
    let client = AppServer::new("/tmp/styra/workspace".into());
    let sent = sent(&client.start());
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["method"], "initialize");
    assert_eq!(sent[0]["id"], INIT_ID);
}

#[test]
fn resume_uses_the_native_thread_resume_method() {
    let client = AppServer::resume("/tmp/styra/workspace".into(), "thread-old".into());
    let sent_lines = sent(&client.handle_line(r#"{"id":1,"result":{}}"#));
    assert_eq!(sent_lines[1]["method"], "thread/resume");
    assert_eq!(sent_lines[1]["params"]["threadId"], "thread-old");
    assert_eq!(sent_lines[1]["params"]["cwd"], "/tmp/styra/workspace");
}

#[test]
fn a_missing_native_thread_is_an_error() {
    let client = AppServer::resume("/tmp/styra/workspace".into(), "gone".into());
    let actions = client.handle_line(r#"{"id":2,"error":{"code":-32602,"message":"not found"}}"#);
    assert!(matches!(
        actions.as_slice(),
        [Action::Error(message)] if message.contains("could not resume Codex thread")
    ));
}

#[test]
fn handshake_progresses_and_a_ready_thread_starts_a_turn() {
    let client = AppServer::new("/tmp/styra/workspace".into());

    // initialize response -> client sends initialized + thread/start
    let sent_lines = sent(&client.handle_line(r#"{"id":1,"result":{}}"#));
    assert_eq!(sent_lines[0]["method"], "initialized");
    assert_eq!(sent_lines[1]["method"], "thread/start");
    assert_eq!(sent_lines[1]["params"]["approvalPolicy"], "never");
    assert_eq!(sent_lines[1]["params"]["sandbox"], "danger-full-access");

    // thread/start response -> ready
    client.handle_line(r#"{"id":2,"result":{"thread":{"id":"thread-xyz"}}}"#);

    // A sent message becomes turn/start referencing the thread.
    let turn = sent(&client.send("do the thing"));
    assert_eq!(turn[0]["method"], "turn/start");
    assert_eq!(turn[0]["params"]["threadId"], "thread-xyz");
    assert_eq!(turn[0]["params"]["input"][0]["text"], "do the thing");
}

#[test]
fn a_turn_can_override_the_model_and_effort() {
    let client = AppServer::new("/tmp/styra/workspace".into());
    client.handle_line(r#"{"id":2,"result":{"thread":{"id":"thread-xyz"}}}"#);

    let turn = sent(&client.send_with_options("continue", Some("gpt-5.6-luna"), Some("low")));
    assert_eq!(turn[0]["params"]["model"], "gpt-5.6-luna");
    assert_eq!(turn[0]["params"]["effort"], "low");
}

#[test]
fn a_changed_directory_is_sent_on_the_next_turn() {
    let client = AppServer::new("/workspace".into());
    client.handle_line(r#"{"id":2,"result":{"thread":{"id":"thread-xyz"}}}"#);
    client.set_cwd("/workspace/crates/styra".into());

    let turn = sent(&client.send("continue"));
    assert_eq!(turn[0]["params"]["cwd"], "/workspace/crates/styra");
}

#[test]
fn interrupt_targets_the_active_turn_and_keeps_the_thread_ready() {
    let client = AppServer::new("/tmp/styra/workspace".into());
    client.handle_line(r#"{"id":2,"result":{"thread":{"id":"thread-xyz"}}}"#);
    client.handle_line(
        r#"{"method":"turn/started","params":{"threadId":"thread-xyz","turn":{"id":"turn-7"}}}"#,
    );

    let interrupt = sent(&client.interrupt().unwrap());
    assert_eq!(interrupt[0]["method"], "turn/interrupt");
    assert_eq!(interrupt[0]["params"]["threadId"], "thread-xyz");
    assert_eq!(interrupt[0]["params"]["turnId"], "turn-7");

    client.handle_line(
            r#"{"method":"turn/completed","params":{"threadId":"thread-xyz","turn":{"id":"turn-7","status":"interrupted"}}}"#,
        );
    assert_eq!(
        client.interrupt().unwrap_err(),
        "there is no active turn to interrupt"
    );
    assert_eq!(sent(&client.send("continue"))[0]["method"], "turn/start");
}

/// A live session must learn the model and effort it is running on from the
/// same line a replayed journal reads it from — and must not then repeat the
/// session as a second, less informative entry when the notification
/// restates the thread.
#[test]
fn the_thread_start_response_surfaces_the_model_and_the_notification_does_not_repeat_it() {
    let client = AppServer::new("/tmp/styra/workspace".into());
    client.handle_line(r#"{"id":1,"result":{}}"#);

    let events = |actions: &[Action]| -> Vec<AgentEvent> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Event(event) => Some(event.clone()),
                _ => None,
            })
            .collect()
    };

    let actions = client.handle_line(
            r#"{"id":2,"result":{"thread":{"id":"t-9"},"model":"gpt-5.6-sol","reasoningEffort":"xhigh"}}"#,
        );
    assert_eq!(
        events(&actions),
        vec![AgentEvent::ThreadStarted {
            thread_id: "t-9".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("xhigh".into()),
        }]
    );

    let actions =
        client.handle_line(r#"{"method":"thread/started","params":{"thread":{"id":"t-9"}}}"#);
    assert!(
        events(&actions).is_empty(),
        "the thread is already known: {actions:?}"
    );
}

#[test]
fn a_message_sent_before_ready_is_queued_then_flushed() {
    let client = AppServer::new("/tmp/styra/workspace".into());

    // Nothing on the wire yet; it was queued.
    assert!(sent(&client.send("early")).is_empty());

    // Becoming ready flushes the queued message as a turn.
    let actions =
        client.handle_line(r#"{"method":"thread/started","params":{"thread":{"id":"t1"}}}"#);
    let turn = sent(&actions);
    assert_eq!(turn[0]["method"], "turn/start");
    assert_eq!(turn[0]["params"]["input"][0]["text"], "early");
}

#[test]
fn a_notification_is_forwarded_as_an_event() {
    let client = AppServer::new("/tmp/styra/workspace".into());
    let actions = client.handle_line(
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m","text":"hi"}}}"#,
        );
    let events: Vec<&AgentEvent> = actions
        .iter()
        .filter_map(|action| match action {
            Action::Event(event) => Some(event),
            _ => None,
        })
        .collect();
    assert_eq!(
        events,
        vec![&AgentEvent::AgentMessage { text: "hi".into() }]
    );
}

#[test]
fn dynamic_tools_are_advertised_and_executed_by_the_host() {
    let tool = DynamicTool::new(
        "make_thing",
        "Make a named thing.",
        json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"],
            "additionalProperties": false,
        }),
        |arguments| {
            let name = arguments
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "name is required".to_owned())?;
            Ok(format!("made {name}"))
        },
    );
    let client = AppServer::new("/tmp/styra/workspace".into()).with_dynamic_tools(vec![tool]);
    let opened = sent(&client.handle_line(r#"{"id":1,"result":{}}"#));
    assert_eq!(opened[1]["params"]["dynamicTools"][0]["name"], "make_thing");
    assert_eq!(
        opened[1]["params"]["dynamicTools"][0]["inputSchema"]["required"][0],
        "name"
    );

    let response = sent(&client.handle_line(
            r#"{"id":"call-7","method":"item/tool/call","params":{"threadId":"t","turnId":"turn","callId":"call","tool":"make_thing","arguments":{"name":"branch"}}}"#,
        ));
    assert_eq!(response[0]["id"], "call-7");
    assert_eq!(response[0]["result"]["success"], true);
    assert_eq!(
        response[0]["result"]["contentItems"][0]["text"],
        "made branch"
    );
}

#[test]
fn a_dynamic_tool_error_is_returned_to_the_agent() {
    let tool = DynamicTool::new("fail", "Fail.", json!({ "type": "object" }), |_| {
        Err("could not do it".into())
    });
    let client = AppServer::new("/tmp/styra/workspace".into()).with_dynamic_tools(vec![tool]);
    let response = sent(&client.handle_line(
        r#"{"id":9,"method":"item/tool/call","params":{"tool":"fail","arguments":{}}}"#,
    ));

    assert_eq!(response[0]["result"]["success"], false);
    assert_eq!(
        response[0]["result"]["contentItems"][0]["text"],
        "could not do it"
    );
}
