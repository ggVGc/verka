//! A Lua client library, generated from this crate's own type definitions.
//!
//! Styra's protocol is deliberately plain JSON over a socket, which makes it
//! easy to speak from anywhere — an editor plugin, a status line, a script.
//! The cost of that reach is transcription: every non-Rust client re-types the
//! operation names, the field names, and the enum spellings by hand, and each
//! copy starts drifting the moment the protocol gains a field. The drift is
//! quiet, too, because `Request` denies unknown fields: a client with one
//! misspelled key gets a rejected request rather than a compiler's complaint.
//!
//! So the bindings are read out of the definitions instead of written beside
//! them. [`SOURCES`] embeds the Rust that declares the wire surface —
//! including the vocabularies Styra borrows from Genta and Driva — with
//! `include_str!`, so what this module parses is exactly the text serde's
//! derives are expanded from, in the same build. There is no separate schema
//! to keep in step, and a protocol change that the generator cannot describe
//! is a failing test rather than a silently stale client.
//!
//! [`model`] reads the sources; [`emit`] writes one Lua module. The result is
//! checked in at [`GENERATED`] for Lua clients to `require`, and a test here
//! fails if it no longer matches what the definitions generate.

mod emit;
mod model;

pub use model::{Body, Field, Model, Payload, Shape, Struct, Tagging, Variant, WireType};

use anyhow::Result;

/// The Rust sources the wire vocabulary is declared in.
///
/// Embedded rather than read from disk so the generator describes the protocol
/// this binary was built from. A file that moves breaks the build, which is
/// the loudest and cheapest moment to find out.
pub const SOURCES: &[(&str, &str)] = &[
    (
        "styra-protocol/src/protocol/mod.rs",
        include_str!("../protocol/mod.rs"),
    ),
    (
        "styra-protocol/src/protocol/types.rs",
        include_str!("../protocol/types.rs"),
    ),
    (
        "genta/src/agent.rs",
        include_str!("../../../genta/src/agent.rs"),
    ),
    (
        "genta/src/event.rs",
        include_str!("../../../genta/src/event.rs"),
    ),
    (
        "driva/src/lib.rs",
        include_str!("../../../driva/src/lib.rs"),
    ),
];

/// Where the wire surface starts. Everything a client ever sends or receives
/// is reachable from these three, so the closure over them is the protocol.
pub const ROOTS: &[&str] = &["Request", "Response", "WireResponse"];

/// The generated library as it is checked in, for callers that want it without
/// regenerating (and for the test that keeps the file honest).
pub const GENERATED: &str = include_str!("../../lua/styra/protocol.lua");

/// Where the checked-in copy lives, relative to the crate root.
pub const GENERATED_PATH: &str = "lua/styra/protocol.lua";

/// Read the protocol out of [`SOURCES`].
pub fn model() -> Result<Model> {
    Model::build(SOURCES, ROOTS)
}

/// Generate the Lua library.
pub fn library() -> Result<String> {
    emit::library(&model()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Contract, Request};
    use std::process::Command;

    fn generated() -> String {
        library().expect("the protocol's own definitions must generate a library")
    }

    /// The point of the whole module: the checked-in Lua is what the current
    /// definitions produce. When this fails, the protocol changed and the
    /// library has not caught up — regenerate it, do not edit it.
    #[test]
    fn the_checked_in_library_is_what_the_definitions_generate() {
        assert_eq!(
            generated(),
            GENERATED,
            "{GENERATED_PATH} is stale; regenerate it with \
             `cargo run -p styra-protocol --bin styra-protocol-lua`"
        );
    }

    /// Every operation is reachable by name, so no client has to fall back to
    /// hand-writing a request table.
    #[test]
    fn every_operation_has_a_constructor() {
        let library = generated();
        let model = model().unwrap();
        let Body::Enum(request) = &model.get("Request").unwrap().body else {
            panic!("Request is an enum of operations");
        };
        assert!(
            request.variants.len() > 30,
            "the operation list looks short"
        );
        for variant in &request.variants {
            assert!(
                library.contains(&format!("function M.request.{}(", variant.name)),
                "{} has no constructor",
                variant.name
            );
        }
    }

    /// The spellings are the generator's own case conversion, so they are
    /// checked against what serde actually writes rather than against
    /// themselves.
    #[test]
    fn enum_spellings_match_what_serde_writes() {
        let library = generated();
        for contract in crate::contract::CONTRACTS {
            let spelling = serde_json::to_value(contract).unwrap();
            assert!(
                library.contains(&format!(
                    "  {} = {:?},",
                    contract.as_str().to_uppercase(),
                    spelling.as_str().unwrap()
                )),
                "{contract:?} is not spelled as serde spells it"
            );
        }
        for provider in crate::agent::Provider::ALL {
            let spelling = serde_json::to_value(provider).unwrap();
            assert_eq!(spelling.as_str().unwrap(), provider.as_str());
            assert!(
                library.contains(&format!("{:?},", provider.as_str())),
                "{provider:?} is missing from the generated spellings"
            );
        }
        // An operation the server matches on, spelled exactly as the request
        // the Rust client sends spells it.
        let request = serde_json::to_value(Request::QuotaLog).unwrap();
        assert_eq!(request["operation"], "quota_log");
        assert!(library.contains("function M.request.quota_log()"));
    }

    /// The vocabularies Styra borrows have to arrive with it: a client that
    /// cannot describe an agent event cannot render an update stream.
    #[test]
    fn the_borrowed_vocabularies_are_generated_too() {
        let model = model().unwrap();
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

    /// A doc comment is the only explanation a Lua client author gets, so the
    /// ones the protocol author wrote have to survive the trip.
    #[test]
    fn rust_doc_comments_reach_the_lua() {
        let library = generated();
        assert!(library.contains("--- Ask the server to remove its socket and exit."));
        assert!(library.contains("--- Fields of `data`:"));
    }

    /// Nullable-but-required is a real distinction on this wire — clearing a
    /// Session name is not the same as not mentioning it — so the descriptors
    /// have to keep the two apart.
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

    /// The generated Lua is Lua. Skipped where no interpreter is installed,
    /// since that is the machine's business and not the protocol's.
    #[test]
    fn the_generated_library_runs_against_a_real_interpreter() {
        let Some(lua) = interpreter() else {
            eprintln!("no lua interpreter on PATH; skipping the behavioural test");
            return;
        };
        let directory = std::env::temp_dir().join(format!("styra-lua-{}", std::process::id()));
        let module = directory.join("styra");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(module.join("protocol.lua"), generated()).unwrap();
        let script = directory.join("check.lua");
        std::fs::write(&script, include_str!("check.lua")).unwrap();

        let output = Command::new(&lua)
            .arg(&script)
            .current_dir(&directory)
            .output()
            .expect("the interpreter must run");
        let _ = std::fs::remove_dir_all(&directory);
        assert!(
            output.status.success(),
            "{lua} rejected the generated library:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The example client is the library's documentation as much as its
    /// README is, and one that no longer loads documents nothing.
    #[test]
    fn the_example_client_loads() {
        let Some(lua) = interpreter() else {
            eprintln!("no lua interpreter on PATH; skipping the example");
            return;
        };
        let lua_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lua");
        for example in ["examples/styra-ask.lua", "examples/json.lua"] {
            let output = Command::new(&lua)
                .arg("-e")
                .arg(format!("assert(loadfile({:?}))", example))
                .current_dir(&lua_root)
                .output()
                .expect("the interpreter must run");
            assert!(
                output.status.success(),
                "{example} does not load:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// The Lua check script builds a request the Rust side then reads back, so
    /// the two halves are held to the same protocol rather than to each other's
    /// descriptions of it.
    #[test]
    fn a_request_built_in_lua_decodes_as_the_rust_request() {
        let Some(lua) = interpreter() else {
            eprintln!("no lua interpreter on PATH; skipping the round trip");
            return;
        };
        let directory = std::env::temp_dir().join(format!("styra-lua-rt-{}", std::process::id()));
        let module = directory.join("styra");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(module.join("protocol.lua"), generated()).unwrap();
        // Encoding by hand keeps the test free of a Lua JSON library, which is
        // not something every machine running these tests will have.
        let script = directory.join("emit.lua");
        std::fs::write(
            &script,
            r#"
local protocol = require("styra.protocol")
local request = protocol.request.send_message({
  id = "styra-1",
  message = { text = "which files handle auth?", contract = protocol.Contract.FILES },
})
print(string.format(
  '{"operation":%q,"data":{"id":%q,"message":{"text":%q,"contract":%q}}}',
  request.operation,
  request.data.id,
  request.data.message.text,
  request.data.message.contract
))
"#,
        )
        .unwrap();
        let output = Command::new(&lua)
            .arg(&script)
            .current_dir(&directory)
            .output()
            .expect("the interpreter must run");
        let _ = std::fs::remove_dir_all(&directory);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let line = String::from_utf8(output.stdout).unwrap();
        let request: Request = serde_json::from_str(line.trim()).expect("valid Styra request");
        let Request::SendMessage { id, message } = request else {
            panic!("the Lua client built the wrong operation");
        };
        assert_eq!(id, "styra-1");
        assert_eq!(message.text, "which files handle auth?");
        assert_eq!(message.contract, Some(Contract::Files));
    }

    fn interpreter() -> Option<String> {
        ["lua", "lua5.4", "lua5.3", "lua5.1", "luajit"]
            .into_iter()
            .find(|name| {
                Command::new(name)
                    .arg("-v")
                    .output()
                    .map(|output| output.status.success())
                    .unwrap_or(false)
            })
            .map(ToOwned::to_owned)
    }
}
