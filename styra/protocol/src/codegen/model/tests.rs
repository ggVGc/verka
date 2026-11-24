use super::*;

#[test]
fn rename_all_matches_serdes_own_case_conversion() {
    assert_eq!(rename("XHigh", Some("lowercase")), "xhigh");
    assert_eq!(rename("BatchMode", Some("kebab-case")), "batch-mode");
    assert_eq!(
        rename("GitRepository", Some("snake_case")),
        "git_repository"
    );
    assert_eq!(rename("ToAgent", Some("snake_case")), "to_agent");
    // Without a style the Rust spelling is the wire spelling.
    assert_eq!(rename("ThroughSelected", None), "ThroughSelected");
    assert_eq!(
        rename("ThroughSelected", Some("camelCase")),
        "throughSelected"
    );
}

#[test]
fn a_type_no_source_defines_is_an_error_naming_who_wanted_it() {
    let source = r#"
            pub struct Root { pub part: Missing }
        "#;
    let error = Model::build(&[("test", source)], &["Root"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("Missing"), "{error}");
    assert!(error.contains("Root"), "{error}");
}

#[test]
fn serde_defaults_are_what_make_a_field_optional() {
    let source = r#"
            pub struct Ask {
                pub id: String,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub note: Option<String>,
                /// Nullable, but the sender still has to say so.
                pub name: Option<String>,
            }
        "#;
    let model = Model::build(&[("test", source)], &["Ask"]).unwrap();
    let Body::Struct(structure) = &model.get("Ask").unwrap().body else {
        panic!("Ask is a struct");
    };
    let required: Vec<_> = structure
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.required))
        .collect();
    assert_eq!(
        required,
        vec![("id", true), ("note", false), ("name", true)]
    );
}

#[test]
fn a_container_default_makes_every_field_optional() {
    let source = r#"
            #[serde(default, deny_unknown_fields)]
            pub struct Policy { pub network: Option<bool>, pub templates: Vec<String> }
        "#;
    let model = Model::build(&[("test", source)], &["Policy"]).unwrap();
    let Body::Struct(structure) = &model.get("Policy").unwrap().body else {
        panic!("Policy is a struct");
    };
    assert!(structure.deny_unknown_fields);
    assert!(structure.fields.iter().all(|field| !field.required));
}

#[test]
fn every_serde_tagging_style_is_recognised() {
    let source = r#"
            #[serde(tag = "operation", content = "data", rename_all = "snake_case")]
            pub enum Adjacent { DoThing, WithData(Payload) }
            #[serde(tag = "type", rename_all = "snake_case")]
            pub enum Internal { OneField { text: String } }
            pub enum External { Bare, Wrapped(Payload) }
            #[serde(untagged)]
            pub enum Untagged { Either(Payload) }
            pub struct Payload { pub text: String }
        "#;
    let model = Model::build(
        &[("test", source)],
        &["Adjacent", "Internal", "External", "Untagged"],
    )
    .unwrap();
    let tagging = |name: &str| match &model.get(name).unwrap().body {
        Body::Enum(enumeration) => enumeration.tagging.clone(),
        Body::Struct(_) => panic!("{name} is an enum"),
    };
    assert_eq!(
        tagging("Adjacent"),
        Tagging::Adjacent {
            tag: "operation".into(),
            content: "data".into()
        }
    );
    assert_eq!(
        tagging("Internal"),
        Tagging::Internal { tag: "type".into() }
    );
    assert_eq!(tagging("External"), Tagging::External);
    assert_eq!(tagging("Untagged"), Tagging::Untagged);
    // The payload was reached through the enums that carry it.
    assert!(model.get("Payload").is_some());
}
