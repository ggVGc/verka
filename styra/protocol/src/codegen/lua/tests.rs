use super::*;
use crate::codegen;
use crate::protocol::{Contract, Request};
use std::process::Command;

/// Where the Lua half of the repository lives — the Svara checkout, found
/// from this crate rather than from the working directory the test was
/// started in.
fn svara() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../svara")
}

fn generated() -> String {
    codegen::generate(&Lua).expect("the protocol's own definitions must generate a library")
}

/// The point of the whole module: the checked-in Lua is what the current
/// definitions produce. When this fails, the protocol changed and the
/// library has not caught up — regenerate it, do not edit it.
#[test]
fn the_checked_in_library_is_what_the_definitions_generate() {
    assert_eq!(
        generated(),
        Lua.generated(),
        "{} is stale; regenerate it with \
             `cargo run -p protocol --bin styra-codegen -- lua`",
        Lua.generated_path()
    );
}

/// Every operation is reachable by name, so no client has to fall back to
/// hand-writing a request table.
#[test]
fn every_operation_has_a_constructor() {
    let library = generated();
    let model = codegen::model().unwrap();
    let Body::Enum(request) = &model.get("Request").unwrap().body else {
        panic!("Request is an enum of operations");
    };
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
        assert!(
            library.contains(&format!("{:?},", provider.as_str())),
            "{provider:?} is missing from the generated spellings"
        );
    }
    // An operation the server matches on, spelled exactly as the request
    // the Rust client sends spells it.
    assert!(library.contains("function M.request.quota_log()"));
}

/// A doc comment is the only explanation a Lua client author gets, so the
/// ones the protocol author wrote have to survive the trip.
#[test]
fn rust_doc_comments_reach_the_lua() {
    let library = generated();
    assert!(library.contains("--- Ask the server to remove its socket and exit."));
    assert!(library.contains("--- Fields of `data`:"));
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

/// The hand-written half of Svara — the client helpers and the command
/// built on them — is the library's documentation as much as its README
/// is, and one that no longer loads documents nothing.
#[test]
fn the_lua_client_and_example_load() {
    let Some(lua) = interpreter() else {
        eprintln!("no lua interpreter on PATH; skipping the example");
        return;
    };
    let root = svara();
    for file in [
        "lua/svara/init.lua",
        "lua/svara/api.lua",
        "lua/svara/core.lua",
        "lua/svara/nvim.lua",
        "lua/svara/protocol.lua",
        "bin/svara",
    ] {
        let output = Command::new(&lua)
            .arg("-e")
            .arg(format!("assert(loadfile({file:?}))"))
            .current_dir(&root)
            .output()
            .expect("the interpreter must run");
        assert!(
            output.status.success(),
            "{file} does not load:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The generated library is not in Svara; it is in this crate, and what
/// reaches it is a search path. `loadfile` above only parses, so it would
/// not notice a path that no longer resolves — this requires the module
/// for real, the way `svara.protocol` does when the plugin runs.
#[test]
fn the_example_finds_the_protocol_where_it_now_lives() {
    let Some(lua) = interpreter() else {
        eprintln!("no lua interpreter on PATH; skipping the search path");
        return;
    };
    let output = Command::new(&lua)
        .arg("-e")
        .arg(
            "package.path = \"lua/?.lua;lua/?/init.lua;\" .. package.path\n\
                 assert(type(require(\"svara.protocol\")) == \"table\")",
        )
        .current_dir(svara())
        .output()
        .expect("the interpreter must run");
    assert!(
        output.status.success(),
        "svara.protocol does not resolve from the Svara checkout:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
