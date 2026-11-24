use super::*;
use crate::codegen;
use crate::protocol::{Contract, Request};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the Elixir half of the repository lives, found from this crate
/// rather than from the working directory the test was started in.
fn styra_elixir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../styra-elixir")
}

fn generated() -> String {
    codegen::generate(&Elixir).expect("the protocol's own definitions must generate a library")
}

/// The point of the whole module: the checked-in Elixir is what the current
/// definitions produce. When this fails, the protocol changed and the
/// library has not caught up — regenerate it, do not edit it.
#[test]
fn the_checked_in_library_is_what_the_definitions_generate() {
    assert_eq!(
        generated(),
        Elixir.generated(),
        "{} is stale; regenerate it with \
             `cargo run -p protocol --bin styra-codegen -- elixir`",
        Elixir.generated_path()
    );
}

/// Every operation is reachable by name, in both conventions, so no client
/// has to fall back to hand-writing a request map.
#[test]
fn every_operation_has_a_constructor() {
    let library = generated();
    let model = codegen::model().unwrap();
    let Body::Enum(request) = &model.get("Request").unwrap().body else {
        panic!("Request is an enum of operations");
    };
    for variant in &request.variants {
        let name = &variant.name;
        let unit = matches!(variant.payload, Payload::Unit);
        let (call, raising) = if unit {
            (format!("def {name}, do:"), format!("def {name}!, do:"))
        } else {
            (format!("def {name}("), format!("def {name}!("))
        };
        assert!(library.contains(&call), "{name} has no constructor");
        assert!(library.contains(&raising), "{name} has no raising form");
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
        let spelling = spelling.as_str().unwrap();
        assert!(
            library.contains(&format!("{{:{spelling}, {spelling:?}}}")),
            "{contract:?} is not spelled as serde spells it"
        );
    }
    for provider in crate::agent::Provider::ALL {
        let spelling = serde_json::to_value(provider).unwrap();
        let spelling = spelling.as_str().unwrap();
        assert!(
            library.contains(&format!("{{{}, {spelling:?}}}", atom(spelling))),
            "{provider:?} is missing from the generated spellings"
        );
    }
    // An operation the server matches on, spelled exactly as the request
    // the Rust client sends spells it.
    assert!(library.contains("def quota_log, do: Styra.Protocol.build(\"quota_log\")"));
}

/// Wire spellings with punctuation require quoted atoms.
#[test]
fn a_spelling_that_is_no_function_name_uses_a_quoted_atom() {
    assert_eq!(atom("batch-mode"), r#":"batch-mode""#);
    assert_eq!(atom("codex"), ":codex");
}

/// A doc comment is the only explanation an Elixir client author gets, so
/// the ones the protocol author wrote have to survive the trip.
#[test]
fn rust_doc_comments_reach_the_elixir() {
    let library = generated();
    assert!(library.contains("Ask the server to remove its socket and exit."));
    assert!(library.contains("Fields of `data`:"));
    assert!(library.contains("@doc ~S\"\"\""));
}

/// `Request` is both the operations enum and the constructor module, and
/// only one of the two can have the name. The constructors win, so the
/// spellings have to remain reachable another way.
#[test]
fn the_operations_are_listed_even_though_request_has_no_enum_module() {
    let library = generated();
    assert!(!library.contains("defmodule Styra.Protocol.Request do"));
    assert!(library.contains("  defmodule Request do"));
    assert!(library.contains("def operations, do: @operations"));
    assert!(library.contains("\"quota_log\""));
}

/// The generated Elixir is Elixir, and it is Elixir the compiler has no
/// complaint about: a warning in a file nobody is allowed to edit is one
/// its readers can only ignore, so it is a failure here instead. Skipped
/// where no interpreter is installed, since that is the machine's business
/// and not the protocol's.
#[test]
fn the_generated_library_compiles_and_runs_against_a_real_interpreter() {
    let Some(elixir) = interpreter("elixir") else {
        eprintln!("no elixir on PATH; skipping the behavioural test");
        return;
    };
    let Some(elixirc) = interpreter("elixirc") else {
        eprintln!("no elixirc on PATH; skipping the behavioural test");
        return;
    };
    let directory = std::env::temp_dir().join(format!("styra-elixir-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let module = directory.join("protocol.ex");
    std::fs::write(&module, generated()).unwrap();

    let compiled = elixir_command(&elixirc)
        .arg("--warnings-as-errors")
        .arg("-o")
        .arg(&directory)
        .arg(&module)
        .output()
        .expect("the compiler must run");
    assert!(
        compiled.status.success(),
        "the generated library does not compile cleanly:\n{}{}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );

    let script = directory.join("check.exs");
    std::fs::write(&script, include_str!("check.exs")).unwrap();
    let output = elixir_command(&elixir)
        .arg("-pa")
        .arg(&directory)
        .arg(&script)
        .output()
        .expect("the interpreter must run");
    let _ = std::fs::remove_dir_all(&directory);
    assert!(
        output.status.success(),
        "{elixir} rejected the generated library:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The hand-written half of `styra-elixir` — the client, and the example
/// built on it — is the library's documentation as much as its README is,
/// and one that no longer compiles documents nothing.
///
/// It is compiled against the generated module in *this* crate, which is
/// the only copy: what `styra-elixir` has is a path dependency on the
/// package here.
///
/// The example is parsed rather than compiled, because compiling a script
/// is running it, and running it wants a server.
#[test]
fn the_elixir_client_compiles_and_the_example_parses() {
    let (Some(elixir), Some(elixirc)) = (interpreter("elixir"), interpreter("elixirc")) else {
        eprintln!("no elixir on PATH; skipping the client");
        return;
    };
    let root = styra_elixir();
    let directory =
        std::env::temp_dir().join(format!("styra-elixir-client-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let compiled = elixir_command(&elixirc)
        .arg("--warnings-as-errors")
        .arg("-o")
        .arg(&directory)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join(Elixir.generated_path()))
        .arg(root.join("lib/styra/client.ex"))
        .output()
        .expect("the compiler must run");
    let _ = std::fs::remove_dir_all(&directory);
    assert!(
        compiled.status.success(),
        "styra-elixir does not compile cleanly:\n{}{}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );

    let example = root.join("examples/styra_ask.exs");
    let parsed = elixir_command(&elixir)
        .arg("-e")
        .arg(format!(
            "Code.string_to_quoted!(File.read!({:?}))",
            example.to_string_lossy()
        ))
        .output()
        .expect("the interpreter must run");
    assert!(
        parsed.status.success(),
        "{} does not parse:\n{}",
        example.display(),
        String::from_utf8_lossy(&parsed.stderr)
    );
}

/// The check script builds a request the Rust side then reads back, so the
/// two halves are held to the same protocol rather than to each other's
/// descriptions of it.
#[test]
fn a_request_built_in_elixir_decodes_as_the_rust_request() {
    let Some(elixir) = interpreter("elixir") else {
        eprintln!("no elixir on PATH; skipping the round trip");
        return;
    };
    let directory = std::env::temp_dir().join(format!("styra-elixir-rt-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let module = directory.join("protocol.ex");
    std::fs::write(&module, generated()).unwrap();
    let script = directory.join("emit.exs");
    std::fs::write(
        &script,
        r#"
{:ok, request} =
  Styra.Protocol.Request.send_message(%{
    id: "styra-1",
    message: %{text: "which files handle auth?", contract: Styra.Protocol.Contract.files()}
  })

IO.puts(JSON.encode!(request))
"#,
    )
    .unwrap();
    let output = elixir_command(&elixir)
        .arg("-r")
        .arg(&module)
        .arg(&script)
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
        panic!("the Elixir client built the wrong operation");
    };
    assert_eq!(id, "styra-1");
    assert_eq!(message.text, "which files handle auth?");
    assert_eq!(message.contract, Some(Contract::Files));
}

/// Elixir reads source as UTF-8 only when the VM is told to, and the
/// warning it prints otherwise would land in the output these tests read.
fn elixir_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env("ELIXIR_ERL_OPTIONS", "+fnu");
    command
}

fn interpreter(program: &str) -> Option<String> {
    elixir_command(program)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|_| program.to_owned())
}
