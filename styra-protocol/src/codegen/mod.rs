//! Client libraries in other languages, generated from this crate's own type
//! definitions.
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
//! The work splits in two, and the split is where a second target language
//! gets in. [`model`] reads the sources into a [`Model`] that knows nothing
//! about any output language: what shapes cross the socket, which fields are
//! required, how each enum is tagged. A [`Language`] then renders that model
//! however its own readers expect. Lua is the first; [`LANGUAGES`] is the list.

pub mod elixir;
pub mod lua;
mod model;

pub use model::{Body, Field, Model, Payload, Shape, Struct, Tagging, Variant, WireType};

use anyhow::{bail, Result};

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

/// One target language.
///
/// A language decides four things, and the four are exactly what differs
/// between an Elixir binding and a Lua one: what the target is called, where
/// its checked-in copy lives, what the current contents of that copy are (so
/// the staleness test can be written once here), and how a [`Model`] reads as
/// a module. Everything upstream of that — parsing the Rust, resolving the
/// type closure, deciding what serde makes of each field, checking the
/// envelope still has the shape every hand-written runtime assumes — is the
/// same work whoever is reading the result, and is done before `render` is
/// called.
///
/// What a backend is therefore left to decide for itself is its own
/// language's business and nobody else's: how an identifier is spelled and
/// which words are reserved, how a comment opens, whether an absent value is
/// `nil` or `null`, and how much of the library is generated tables versus a
/// hand-written runtime it ships alongside.
pub trait Language {
    /// How the language is named on the command line.
    fn name(&self) -> &'static str;

    /// Where the checked-in copy lives, relative to this crate's root.
    fn generated_path(&self) -> &'static str;

    /// The checked-in copy as it currently stands, for callers that want it
    /// without regenerating and for the test that keeps the file honest.
    fn generated(&self) -> &'static str;

    /// Render the model as one module.
    fn render(&self, model: &Model) -> Result<String>;
}

/// Every language a client library can be generated for.
pub const LANGUAGES: &[&dyn Language] = &[&lua::Lua, &elixir::Elixir];

/// Look a language up by the name it is asked for on the command line.
pub fn language(name: &str) -> Result<&'static dyn Language> {
    LANGUAGES
        .iter()
        .find(|language| language.name() == name)
        .copied()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no generator for {name}; styra-protocol generates: {}",
                names().join(", ")
            )
        })
}

/// The names of every language, for help text and error messages.
pub fn names() -> Vec<&'static str> {
    LANGUAGES.iter().map(|language| language.name()).collect()
}

/// Read the protocol out of [`SOURCES`].
pub fn model() -> Result<Model> {
    let model = Model::build(SOURCES, ROOTS)?;
    check_envelope(&model)?;
    Ok(model)
}

/// Generate the client library for one language.
pub fn generate(language: &dyn Language) -> Result<String> {
    language.render(&model()?)
}

/// What every generated library takes for granted.
///
/// A runtime spells `ok`, `error`, `operation` and `data` by hand, in whatever
/// language it is written in, because a helper that unwraps a response reads
/// better than one parameterised over how responses are tagged. That is only
/// safe while the protocol really is tagged that way, so no library is emitted
/// at all once the envelope has moved out from under it. The check is here
/// rather than in a backend because the assumption is not Lua's; it is made
/// again by every language that ships a runtime.
fn check_envelope(model: &Model) -> Result<()> {
    let Some(request) = model.get("Request") else {
        bail!("the model has no Request type to build requests from");
    };
    match &request.body {
        Body::Enum(enumeration) => match &enumeration.tagging {
            Tagging::Adjacent { tag, content } if tag == "operation" && content == "data" => {}
            other => bail!(
                "Request is tagged {other:?}; generated libraries build \
                 {{operation, data}} and would be wrong"
            ),
        },
        Body::Struct(_) => bail!("Request is no longer an enum of operations"),
    }
    let Some(wire) = model.get("WireResponse") else {
        bail!("the model has no WireResponse envelope");
    };
    let Body::Enum(envelope) = &wire.body else {
        bail!("WireResponse is no longer an enum");
    };
    if envelope.tagging
        != (Tagging::Internal {
            tag: "status".into(),
        })
    {
        bail!(
            "WireResponse is tagged {:?}; a generated unwrap reads a `status` \
             field and would be wrong",
            envelope.tagging
        );
    }
    let spellings: Vec<&str> = envelope
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect();
    if spellings != ["ok", "error"] {
        bail!("WireResponse spells its outcomes {spellings:?}, not [\"ok\", \"error\"]");
    }
    Ok(())
}

/// A Rust doc comment, ready to be written as a comment in any language.
///
/// Rustdoc's intra-doc links are unwrapped to the plain name they point at: a
/// reader outside Rust cannot follow ``[`Contract`]`` anywhere, and the
/// brackets only get in the way of the sentence. The comment marker is the
/// caller's, since that is the part that is actually per-language.
pub fn prose(line: &str) -> String {
    line.replace("[`", "`").replace("`]", "`")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vocabularies Styra borrows have to arrive with it: a client that
    /// cannot describe an agent event cannot render an update stream.
    #[test]
    fn the_borrowed_vocabularies_are_in_the_model() {
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
}
