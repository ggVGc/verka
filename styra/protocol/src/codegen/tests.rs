use super::*;

/// The vocabularies Styra borrows have to arrive with it: a client that
/// cannot describe an agent event cannot render an update stream.
#[test]
fn the_borrowed_vocabularies_are_in_the_model() {
    let model = model().unwrap();
    let Body::Enum(request) = &model.get("Request").unwrap().body else {
        panic!("Request is an enum of operations");
    };
    assert!(
        request.variants.len() > 30,
        "the operation list looks short"
    );
    for provider in crate::agent::Provider::ALL {
        assert_eq!(serde_json::to_value(provider).unwrap(), provider.as_str());
    }
    assert_eq!(
        serde_json::to_value(crate::protocol::Request::QuotaLog).unwrap()["operation"],
        "quota_log"
    );
    for name in [
        "AgentEvent",  // genta
        "Selection",   // genta
        "Provider",    // genta
        "Mount",       // driva
        "MountAccess", // driva
        "Answer",      // styra-protocol
        "InteractionUpdate",
    ] {
        assert!(
            model.get(name).is_some(),
            "{name} is missing from the model"
        );
    }
}

/// Nullable-but-required is a real distinction on this wire — clearing a
/// Session name is not the same as not mentioning it — so the model has to
/// keep the two apart for every language that reads it.
#[test]
fn a_nullable_field_is_still_a_required_one() {
    let model = model().unwrap();
    let Body::Struct(rename) = &model.get("RenameSession").unwrap().body else {
        panic!("RenameSession is a struct");
    };
    let name = rename
        .fields
        .iter()
        .find(|field| field.name == "name")
        .expect("the rename carries a name");
    assert!(name.required);
    assert!(matches!(name.shape, Shape::Optional(_)));
}

/// The envelope every runtime hand-writes is the one the protocol still
/// has. [`model`] runs the check, so a protocol that moved fails here
/// rather than in each backend's own tests.
#[test]
fn the_envelope_is_what_the_runtimes_assume() {
    let model = Model::build(SOURCES, ROOTS).unwrap();
    check_envelope(&model).expect("the envelope has moved; see check_envelope");
}

/// A language is reachable by the name its users type, and an unknown one
/// says what there is rather than just failing.
#[test]
fn languages_are_looked_up_by_name() {
    assert_eq!(language("lua").unwrap().name(), "lua");
    let error = match language("cobol") {
        Ok(found) => panic!("{} answered to cobol", found.name()),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("cobol"), "{error}");
    assert!(error.contains("lua"), "{error}");
}
