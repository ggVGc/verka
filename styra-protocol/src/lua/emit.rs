//! Writing a [`Model`] out as one Lua module.
//!
//! The module is data first and code second: every wire type becomes a
//! descriptor in `M.types`, and the handful of functions a client actually
//! calls are driven by those descriptors rather than generated one per
//! operation. What *is* generated per operation is a named constructor with
//! the Rust doc comment above it, because a client author reading
//! `M.request.branch_session` should find the same explanation the protocol
//! author wrote, not a note telling them to go and read the Rust.

use super::model::{shouted, Body, Field, Model, Payload, Shape, Struct, Tagging, WireType};
use anyhow::{bail, Result};
use std::fmt::Write;

/// Lua support the generated tables are useless without.
const RUNTIME: &str = include_str!("runtime.lua");

/// Render the whole library.
pub fn library(model: &Model) -> Result<String> {
    check_assumptions(model)?;
    let mut out = String::new();
    header(&mut out);
    writeln!(out, "local M = {{}}\n")?;
    answers(&mut out)?;
    types(&mut out, model)?;
    enums(&mut out, model)?;
    section(&mut out, "Runtime");
    writeln!(out, "{}", RUNTIME.trim_end())?;
    requests(&mut out, model)?;
    writeln!(out, "\nreturn M")?;
    Ok(out)
}

/// What the hand-written half of the library takes for granted.
///
/// [`RUNTIME`] spells `ok`, `error`, `operation` and `data` by hand, because a
/// helper that unwraps a response reads better than a helper parameterised
/// over how responses are tagged. That is only safe while the protocol really
/// is tagged that way, so the generator refuses to emit a library whose
/// envelope has moved out from under it.
fn check_assumptions(model: &Model) -> Result<()> {
    let Some(request) = model.get("Request") else {
        bail!("the model has no Request type to build requests from");
    };
    match &request.body {
        Body::Enum(enumeration) => match &enumeration.tagging {
            Tagging::Adjacent { tag, content } if tag == "operation" && content == "data" => {}
            other => bail!(
                "Request is tagged {other:?}; the generated library builds \
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
            "WireResponse is tagged {:?}; the generated unwrap reads a `status` \
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

fn header(out: &mut String) {
    out.push_str(
        "-- The Styra client/server wire vocabulary, as a Lua module.\n\
         --\n\
         -- Generated from the Serde type definitions in `styra-protocol`; every\n\
         -- operation, field name, and enum spelling here is read out of the Rust that\n\
         -- defines the protocol, so this file cannot describe a protocol the server\n\
         -- does not speak. Do not edit it by hand.\n\
         --\n\
         --   cargo run -p styra-protocol --bin styra-protocol-lua\n\
         --\n\
         -- It carries no transport and no JSON codec, exactly as the Rust crate does\n\
         -- not: a request is a plain Lua table for your own encoder to serialise and\n\
         -- your own socket to carry, one JSON object per line.\n\
         --\n\
         --   local protocol = require(\"styra.protocol\")\n\
         --   protocol.use_null(json.null)\n\
         --   local line = json.encode(protocol.request.send_message({\n\
         --     id = session, message = { text = \"hello\" },\n\
         --   }))\n\
         --   local answer, err = protocol.expect(json.decode(reply), protocol.Response.ANSWER)\n\
         --\n",
    );
}

fn section(out: &mut String, title: &str) {
    out.push_str(&format!(
        "\n-- {}\n-- {}\n\n",
        title,
        "-".repeat(title.len().max(3))
    ));
}

/// The answer delimiters, which a client parsing a contract's block itself
/// (rather than asking `turn_answer` for it) has to agree with exactly.
fn answers(out: &mut String) -> Result<()> {
    section(out, "Typed answers");
    writeln!(
        out,
        "--- The delimiters a contract's answer block sits in.\n\
         M.ANSWER_OPEN = {:?}\n\
         M.ANSWER_CLOSE = {:?}",
        crate::contract::OPEN,
        crate::contract::CLOSE
    )?;
    Ok(())
}

fn types(out: &mut String, model: &Model) -> Result<()> {
    section(out, "Wire types");
    out.push_str(
        "--- Every type that crosses the socket, described well enough to check a\n\
         --- value against it. See `M.validate`.\n\
         M.types = {}\n",
    );
    for wire in &model.types {
        out.push('\n');
        docs(out, &wire.docs, "");
        write!(out, "M.types.{} = ", wire.name)?;
        out.push_str(&descriptor(wire));
        out.push('\n');
    }
    Ok(())
}

fn descriptor(wire: &WireType) -> String {
    let mut out = String::new();
    match &wire.body {
        Body::Struct(structure) => {
            out.push_str("{\n  kind = \"struct\",\n");
            if structure.deny_unknown_fields {
                out.push_str("  deny_unknown_fields = true,\n");
            }
            out.push_str("  fields = ");
            out.push_str(&fields(&structure.fields, 2));
            out.push_str(",\n}");
        }
        Body::Enum(enumeration) => {
            out.push_str("{\n  kind = \"enum\",\n");
            let tagging = match &enumeration.tagging {
                Tagging::External => "{ style = \"external\" }".to_owned(),
                Tagging::Internal { tag } => {
                    format!("{{ style = \"internal\", tag = {tag:?} }}")
                }
                Tagging::Adjacent { tag, content } => {
                    format!("{{ style = \"adjacent\", tag = {tag:?}, content = {content:?} }}")
                }
                Tagging::Untagged => "{ style = \"untagged\" }".to_owned(),
            };
            out.push_str(&format!("  tagging = {tagging},\n"));
            // A value is a bare string only when every variant is a unit *and*
            // nothing wraps it: an internally tagged enum of unit variants is
            // still an object carrying its tag.
            if wire.is_plain_enum() && enumeration.tagging == Tagging::External {
                out.push_str("  plain = true,\n");
            }
            if enumeration.deny_unknown_fields {
                out.push_str("  deny_unknown_fields = true,\n");
            }
            out.push_str("  variants = {\n");
            for variant in &enumeration.variants {
                out.push_str(&format!("    {{ name = {:?}, payload = ", variant.name));
                out.push_str(&payload(&variant.payload, 4));
                out.push_str(" },\n");
            }
            out.push_str("  },\n}");
        }
    }
    out
}

fn payload(payload: &Payload, indent: usize) -> String {
    match payload {
        Payload::Unit => "{ kind = \"unit\" }".to_owned(),
        Payload::Newtype(shape) => format!("{{ kind = \"newtype\", type = {} }}", shape_of(shape)),
        Payload::Tuple(shapes) => format!(
            "{{ kind = \"tuple\", items = {{ {} }} }}",
            shapes.iter().map(shape_of).collect::<Vec<_>>().join(", ")
        ),
        Payload::Struct(structure) => {
            let pad = " ".repeat(indent);
            let mut out = format!("{{\n{pad}  kind = \"struct\",\n");
            if structure.deny_unknown_fields {
                out.push_str(&format!("{pad}  deny_unknown_fields = true,\n"));
            }
            out.push_str(&format!("{pad}  fields = "));
            out.push_str(&fields(&structure.fields, indent + 2));
            out.push_str(&format!(",\n{pad}}}"));
            out
        }
    }
}

fn fields(fields: &[Field], indent: usize) -> String {
    if fields.is_empty() {
        return "{}".to_owned();
    }
    let pad = " ".repeat(indent);
    let mut out = String::from("{\n");
    for field in fields {
        out.push_str(&format!(
            "{pad}  {{ name = {:?}, required = {}, type = {} }},\n",
            field.name,
            field.required,
            shape_of(&field.shape)
        ));
    }
    out.push_str(&format!("{pad}}}"));
    out
}

fn shape_of(shape: &Shape) -> String {
    match shape {
        Shape::Bool => "{ kind = \"boolean\" }".into(),
        Shape::Integer => "{ kind = \"number\", integer = true }".into(),
        Shape::Number => "{ kind = \"number\" }".into(),
        Shape::Text => "{ kind = \"string\" }".into(),
        Shape::Path => "{ kind = \"string\", path = true }".into(),
        Shape::Json => "{ kind = \"any\" }".into(),
        Shape::List(item) => format!("{{ kind = \"list\", item = {} }}", shape_of(item)),
        Shape::Map(value) => format!("{{ kind = \"map\", value = {} }}", shape_of(value)),
        Shape::Optional(inner) => format!("{{ kind = \"optional\", inner = {} }}", shape_of(inner)),
        Shape::Ref(name) => format!("{{ kind = \"ref\", name = {name:?} }}"),
    }
}

/// Wire spellings as named constants, so a client compares against
/// `M.Response.ANSWER` rather than retyping `"answer"` at each call site.
fn enums(out: &mut String, model: &Model) -> Result<()> {
    section(out, "Enum spellings");
    out.push_str(
        "--- The wire spellings of every enum, in declaration order.\n\
         M.enums = {}\n",
    );
    for wire in &model.types {
        let Body::Enum(enumeration) = &wire.body else {
            continue;
        };
        let names: Vec<String> = enumeration
            .variants
            .iter()
            .map(|variant| format!("{:?}", variant.name))
            .collect();
        writeln!(out, "\nM.enums.{} = {{ {} }}", wire.name, names.join(", "))?;
        writeln!(out, "--- Wire spellings of `{}`.", wire.name)?;
        writeln!(out, "M.{} = {{", wire.name)?;
        for variant in &enumeration.variants {
            writeln!(out, "  {} = {:?},", shouted(&variant.ident), variant.name)?;
        }
        writeln!(out, "}}")?;
    }
    writeln!(
        out,
        "\n--- Every operation the server answers, in protocol order.\nM.OPERATIONS = M.enums.Request"
    )?;
    Ok(())
}

/// One named constructor per operation, which is what a client writes against.
fn requests(out: &mut String, model: &Model) -> Result<()> {
    section(out, "Requests");
    out.push_str(
        "--- One constructor per operation. Each checks what it is given and returns\n\
         --- the request table to encode and send.\n\
         M.request = {}\n",
    );
    let Some(Body::Enum(request)) = model.get("Request").map(|wire| &wire.body) else {
        bail!("Request is not an enum of operations");
    };
    for variant in &request.variants {
        let name = &variant.name;
        if !is_lua_name(name) {
            bail!(
                "the {name} operation cannot be spelled as a Lua field; rename it \
                 on the wire or teach the generator to bracket it"
            );
        }
        out.push('\n');
        docs(out, &variant.docs, "");
        match &variant.payload {
            Payload::Unit => {
                writeln!(out, "function M.request.{name}()")?;
                writeln!(out, "  return M.build({name:?})")?;
                writeln!(out, "end")?;
            }
            Payload::Newtype(Shape::Ref(referenced)) => {
                let structure = match model.get(referenced).map(|wire| &wire.body) {
                    Some(Body::Struct(structure)) => Some(structure),
                    _ => None,
                };
                if let Some(structure) = structure {
                    field_docs(out, structure, !variant.docs.is_empty());
                }
                writeln!(out, "function M.request.{name}(data)")?;
                writeln!(out, "  return M.build({name:?}, data)")?;
                writeln!(out, "end")?;
            }
            Payload::Newtype(shape) => {
                writeln!(out, "--- Takes one {} value.", shape.label())?;
                writeln!(out, "function M.request.{name}(value)")?;
                writeln!(out, "  return M.build({name:?}, value)")?;
                writeln!(out, "end")?;
            }
            Payload::Tuple(shapes) => {
                let labels: Vec<String> = shapes.iter().map(Shape::label).collect();
                writeln!(out, "--- Takes a list of {}.", labels.join(", then "))?;
                writeln!(out, "function M.request.{name}(values)")?;
                writeln!(out, "  return M.build({name:?}, values)")?;
                writeln!(out, "end")?;
            }
            Payload::Struct(structure) => {
                field_docs(out, structure, !variant.docs.is_empty());
                writeln!(out, "function M.request.{name}(data)")?;
                writeln!(out, "  return M.build({name:?}, data)")?;
                writeln!(out, "end")?;
            }
        }
    }
    Ok(())
}

/// Whether a wire name can be written as `M.request.<name>`.
fn is_lua_name(name: &str) -> bool {
    const KEYWORDS: [&str; 22] = [
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
        "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    !KEYWORDS.contains(&name)
        && !name.is_empty()
        && !name.starts_with(|character: char| character.is_ascii_digit())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// The fields of a request's data, as a table in its doc comment. `separate`
/// only when something was written above it.
fn field_docs(out: &mut String, structure: &Struct, separate: bool) {
    if structure.fields.is_empty() {
        return;
    }
    let width = structure
        .fields
        .iter()
        .map(|field| field.name.len())
        .max()
        .unwrap_or(0);
    if separate {
        out.push_str("---\n");
    }
    out.push_str("--- Fields of `data`:\n");
    for field in &structure.fields {
        out.push_str(&format!(
            "---   {:width$}  {}{}\n",
            field.name,
            field.shape.label(),
            if field.required { "" } else { "  (optional)" },
        ));
    }
}

/// Rust doc comments, carried through as Lua ones.
///
/// Rustdoc's intra-doc links are unwrapped to the plain name they point at: a
/// Lua reader cannot follow `[`Contract`]` anywhere, and the brackets only get
/// in the way of the sentence.
fn docs(out: &mut String, docs: &[String], indent: &str) {
    for line in docs {
        let line = line.replace("[`", "`").replace("`]", "`");
        if line.is_empty() {
            out.push_str(&format!("{indent}---\n"));
        } else {
            out.push_str(&format!("{indent}--- {line}\n"));
        }
    }
}
