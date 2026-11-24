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
fn changing_model_waits_for_ack_before_sending_the_turn() {
    let client = ClaudeStream::new("claude-sonnet-5".into());
    let control = sent(&client.send("use the stronger model", Some("claude-opus-5")));
    assert_eq!(control[0]["request"]["subtype"], "set_model");
    assert_eq!(control[0]["request"]["model"], "claude-opus-5");

    let actions = client
            .handle_line(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"styra_model_1","response":{}}}"#,
            )
            .unwrap();
    let turn = sent(&actions);
    assert_eq!(turn[0]["type"], "user");
    assert_eq!(turn[0]["message"]["content"], "use the stronger model");
}

#[test]
fn interrupting_asks_claude_to_abandon_the_turn_and_completes_it() {
    let client = ClaudeStream::new("claude-sonnet-5".into());
    let request = sent(&client.interrupt());
    assert_eq!(request[0]["type"], "control_request");
    assert_eq!(request[0]["request"]["subtype"], "interrupt");

    let actions = client
            .handle_line(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"styra_interrupt_1","response":{}}}"#,
            )
            .unwrap();
    assert!(matches!(
        actions.as_slice(),
        [
            Action::Info(_),
            Action::Event(AgentEvent::TurnCompleted { .. })
        ]
    ));
}

#[test]
fn switching_model_between_turns_changes_it_without_a_turn_to_release() {
    let client = ClaudeStream::new("claude-sonnet-5".into());
    let control = sent(&client.set_model("claude-opus-5"));
    assert_eq!(control[0]["request"]["subtype"], "set_model");

    let actions = client
            .handle_line(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"styra_model_1","response":{}}}"#,
            )
            .unwrap();
    assert!(matches!(actions.as_slice(), [Action::Info(_)]));
    // The change stuck, so a later turn needs no control traffic at all.
    let turn = sent(&client.send("continue", Some("claude-opus-5")));
    assert_eq!(turn[0]["type"], "user");
}

#[test]
fn a_turn_typed_during_a_pending_switch_rides_along_with_it() {
    let client = ClaudeStream::new("claude-sonnet-5".into());
    client.set_model("claude-opus-5");
    // No second set_model: the turn attaches to the change already in flight.
    assert!(sent(&client.send("go", Some("claude-opus-5"))).is_empty());

    let actions = client
            .handle_line(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"styra_model_1","response":{}}}"#,
            )
            .unwrap();
    let turn = sent(&actions);
    assert_eq!(turn[0]["message"]["content"], "go");
}

#[test]
fn a_rejected_bare_switch_warns_without_completing_a_turn() {
    let client = ClaudeStream::new("claude-sonnet-5".into());
    client.set_model("claude-opus-5");

    let actions = client
            .handle_line(
                r#"{"type":"control_response","response":{"subtype":"error","request_id":"styra_model_1","error":"no such model"}}"#,
            )
            .unwrap();
    assert!(matches!(actions.as_slice(), [Action::Warn(_)]));
}

#[test]
fn switching_to_the_current_model_asks_claude_for_nothing() {
    let client = ClaudeStream::new("claude-sonnet-5".into());
    assert!(client.set_model("claude-sonnet-5").is_empty());
}

#[test]
fn the_current_model_sends_without_control_traffic() {
    let client = ClaudeStream::new("claude-sonnet-5".into());
    let turn = sent(&client.send("continue", Some("claude-sonnet-5")));
    assert_eq!(turn[0]["type"], "user");
}
