//! Writing a [`Model`] out as one Elixir module.
//!
//! The shape is the Lua library's, because the protocol's is: every wire type
//! becomes a descriptor in `types/0`, the handful of functions a client calls
//! are driven by those descriptors, and what is generated per operation is a
//! named function carrying the Rust doc comment the protocol author wrote.
//!
//! What differs is everything Elixir has its own opinion about, which is the
//! point of a backend being a backend:
//!
//! - No null sentinel. A Lua table cannot tell an absent key from one set to
//!   `nil`, so the Lua library needs `protocol.null`; an Elixir map can, so
//!   nullable-but-required is said by leaving the key out or setting it to
//!   `nil` and the sentinel does not exist.
//! - Errors are returned, not raised: `{:ok, request}` and `{:error, message}`
//!   with a `!` form beside each for callers who would rather it raised. Lua
//!   has one convention and Elixir has both.
//! - Enum spellings become a module apiece — `Styra.Protocol.Contract.files()`
//!   — with `parse/1` and `values/0`, since a bare table of constants is not
//!   how Elixir hands a vocabulary to its callers.
//! - Requests come out with string keys, which is what a JSON encoder wants,
//!   while the data a caller passes in may use atom keys, which is what
//!   writing Elixir by hand wants.

use super::model::{Body, Field, Model, Payload, Shape, Struct, Tagging, Variant, WireType};
use super::Language;
use anyhow::{bail, Result};
use std::fmt::Write;

/// Elixir support the generated descriptors are useless without.
const RUNTIME: &str = include_str!("runtime.ex");

/// The module everything is generated under.
const MODULE: &str = "Styra.Protocol";

/// The Elixir client library.
pub struct Elixir;

impl Language for Elixir {
    fn name(&self) -> &'static str {
        "elixir"
    }

    /// Inside this crate, not in `styra-elixir`, so the protocol has one home.
    /// `styra-elixir` depends on the package here rather than carrying a copy.
    fn generated_path(&self) -> &'static str {
        "elixir/lib/styra/protocol.ex"
    }

    fn generated(&self) -> &'static str {
        include_str!("../../../elixir/lib/styra/protocol.ex")
    }

    fn render(&self, model: &Model) -> Result<String> {
        library(model)
    }
}

/// Render the whole library.
fn library(model: &Model) -> Result<String> {
    let mut out = String::new();
    header(&mut out);
    writeln!(out, "defmodule {MODULE} do")?;
    moduledoc(&mut out)?;
    answers(&mut out)?;
    types(&mut out, model)?;
    operations(&mut out, model)?;
    section(&mut out, "Runtime");
    writeln!(out, "{}", RUNTIME.trim_end())?;
    requests(&mut out, model)?;
    writeln!(out, "end")?;
    enums(&mut out, model)?;
    Ok(out)
}

fn header(out: &mut String) {
    out.push_str(
        "# The Styra client/server wire vocabulary, as an Elixir module.\n\
         #\n\
         # Generated from the Serde type definitions in `styra/protocol`; every\n\
         # operation, field name, and enum spelling here is read out of the Rust that\n\
         # defines the protocol, so this file cannot describe a protocol the server\n\
         # does not speak. Do not edit it by hand.\n\
         #\n\
         #   cargo run -p protocol --bin styra-codegen -- elixir\n\
         #\n\
         # It carries no transport and no JSON codec, exactly as the Rust crate does\n\
         # not: a request is a plain map for your own encoder to serialise and your\n\
         # own socket to carry, one JSON object per line.\n\
         #\n\
         #   {:ok, request} = Styra.Protocol.Request.send_message(%{\n\
         #     id: session,\n\
         #     message: %{text: \"hello\"}\n\
         #   })\n\
         #   {:ok, answer} = Styra.Protocol.expect(reply, Styra.Protocol.Response.answer())\n\
         #\n\n",
    );
}

fn moduledoc(out: &mut String) -> Result<()> {
    writeln!(
        out,
        "  @moduledoc ~S\"\"\"\n\
         \x20 The Styra client/server wire vocabulary.\n\
         \x20\n\
         \x20 Generated from the Serde type definitions in `styra/protocol`, so every\n\
         \x20 operation, field name, and enum spelling is the one the server speaks.\n\
         \x20\n\
         \x20 A request is a plain map; carrying it is the caller's business — one JSON\n\
         \x20 object per line over the server's Unix socket. `Styra.Protocol.Request`\n\
         \x20 builds them, `validate/2` checks any wire type, and `unwrap/1` and\n\
         \x20 `expect/2` read what comes back.\n\
         \x20 \"\"\"\n"
    )?;
    Ok(())
}

fn section(out: &mut String, title: &str) {
    out.push_str(&format!(
        "\n  # {}\n  # {}\n\n",
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
        "  @doc \"The opening delimiter a contract's answer block sits in.\"\n\
         \x20 def answer_open, do: {:?}\n\n\
         \x20 @doc \"The closing delimiter a contract's answer block sits in.\"\n\
         \x20 def answer_close, do: {:?}",
        crate::contract::OPEN,
        crate::contract::CLOSE
    )?;
    Ok(())
}

fn types(out: &mut String, model: &Model) -> Result<()> {
    section(out, "Wire types");
    let mut entries = Vec::new();
    for wire in &model.types {
        let mut entry = String::new();
        comment(&mut entry, &wire.docs, "    ");
        write!(entry, "    {:?} => {}", wire.name, descriptor(wire, 4))?;
        entries.push(entry);
    }
    writeln!(out, "  @types %{{\n{}\n  }}\n", entries.join(",\n\n"))?;
    writeln!(
        out,
        "  @doc ~S\"\"\"\n\
         \x20 Every type that crosses the socket, described well enough to check a value\n\
         \x20 against it. See `validate/2`.\n\
         \x20 \"\"\"\n\
         \x20 @spec types() :: %{{String.t() => map()}}\n\
         \x20 def types, do: @types\n\n\
         \x20 @doc \"The descriptor for one wire type, or nil.\"\n\
         \x20 @spec type(String.t()) :: map() | nil\n\
         \x20 def type(name), do: Map.get(@types, name)"
    )?;
    Ok(())
}

fn operations(out: &mut String, model: &Model) -> Result<()> {
    let Some(Body::Enum(request)) = model.get("Request").map(|wire| &wire.body) else {
        bail!("Request is not an enum of operations");
    };
    let names: Vec<String> = request
        .variants
        .iter()
        .map(|variant| format!("{:?}", variant.name))
        .collect();
    section(out, "Operations");
    writeln!(
        out,
        "  @operations [\n    {}\n  ]\n\n\
         \x20 @doc \"Every operation the server answers, in protocol order.\"\n\
         \x20 @spec operations() :: [String.t()]\n\
         \x20 def operations, do: @operations",
        names.join(",\n    ")
    )?;
    Ok(())
}

/// A map literal describing one wire type.
fn descriptor(wire: &WireType, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let mut out = String::from("%{\n");
    match &wire.body {
        Body::Struct(structure) => {
            out.push_str(&format!("{pad}  kind: :struct,\n"));
            if structure.deny_unknown_fields {
                out.push_str(&format!("{pad}  deny_unknown_fields: true,\n"));
            }
            out.push_str(&format!(
                "{pad}  fields: {}\n",
                fields(&structure.fields, indent + 2)
            ));
        }
        Body::Enum(enumeration) => {
            out.push_str(&format!("{pad}  kind: :enum,\n"));
            let tagging = match &enumeration.tagging {
                Tagging::External => "%{style: :external}".to_owned(),
                Tagging::Internal { tag } => format!("%{{style: :internal, tag: {tag:?}}}"),
                Tagging::Adjacent { tag, content } => {
                    format!("%{{style: :adjacent, tag: {tag:?}, content: {content:?}}}")
                }
                Tagging::Untagged => "%{style: :untagged}".to_owned(),
            };
            out.push_str(&format!("{pad}  tagging: {tagging},\n"));
            // A value is a bare string only when every variant is a unit *and*
            // nothing wraps it: an internally tagged enum of unit variants is
            // still a map carrying its tag.
            if wire.is_plain_enum() && enumeration.tagging == Tagging::External {
                out.push_str(&format!("{pad}  plain: true,\n"));
            }
            if enumeration.deny_unknown_fields {
                out.push_str(&format!("{pad}  deny_unknown_fields: true,\n"));
            }
            let variants: Vec<String> = enumeration
                .variants
                .iter()
                .map(|variant| {
                    format!(
                        "{pad}    %{{name: {:?}, payload: {}}}",
                        variant.name,
                        payload(&variant.payload, indent + 4)
                    )
                })
                .collect();
            if variants.is_empty() {
                out.push_str(&format!("{pad}  variants: []\n"));
            } else {
                out.push_str(&format!(
                    "{pad}  variants: [\n{}\n{pad}  ]\n",
                    variants.join(",\n")
                ));
            }
        }
    }
    out.push_str(&format!("{pad}}}"));
    out
}

fn payload(payload: &Payload, indent: usize) -> String {
    match payload {
        Payload::Unit => "%{kind: :unit}".to_owned(),
        Payload::Newtype(shape) => format!("%{{kind: :newtype, type: {}}}", shape_of(shape)),
        Payload::Tuple(shapes) => format!(
            "%{{kind: :tuple, items: [{}]}}",
            shapes.iter().map(shape_of).collect::<Vec<_>>().join(", ")
        ),
        Payload::Struct(structure) => {
            let pad = " ".repeat(indent);
            let mut out = format!("%{{\n{pad}  kind: :struct,\n");
            if structure.deny_unknown_fields {
                out.push_str(&format!("{pad}  deny_unknown_fields: true,\n"));
            }
            out.push_str(&format!(
                "{pad}  fields: {}\n{pad}}}",
                fields(&structure.fields, indent + 2)
            ));
            out
        }
    }
}

fn fields(fields: &[Field], indent: usize) -> String {
    if fields.is_empty() {
        return "[]".to_owned();
    }
    let pad = " ".repeat(indent);
    let entries: Vec<String> = fields
        .iter()
        .map(|field| {
            format!(
                "{pad}  %{{name: {:?}, required: {}, type: {}}}",
                field.name,
                field.required,
                shape_of(&field.shape)
            )
        })
        .collect();
    format!("[\n{}\n{pad}]", entries.join(",\n"))
}

fn shape_of(shape: &Shape) -> String {
    match shape {
        Shape::Bool => "%{kind: :boolean}".into(),
        Shape::Integer => "%{kind: :number, integer: true}".into(),
        Shape::Number => "%{kind: :number}".into(),
        Shape::Text => "%{kind: :string}".into(),
        Shape::Path => "%{kind: :string, path: true}".into(),
        Shape::Json => "%{kind: :any}".into(),
        Shape::List(item) => format!("%{{kind: :list, item: {}}}", shape_of(item)),
        Shape::Map(value) => format!("%{{kind: :map, value: {}}}", shape_of(value)),
        Shape::Optional(inner) => format!("%{{kind: :optional, inner: {}}}", shape_of(inner)),
        Shape::Ref(name) => format!("%{{kind: :ref, name: {name:?}}}"),
    }
}

/// One function per operation, which is what a client writes against, in a
/// module of their own so `Request.send_message` reads as what it is.
fn requests(out: &mut String, model: &Model) -> Result<()> {
    let Some(Body::Enum(request)) = model.get("Request").map(|wire| &wire.body) else {
        bail!("Request is not an enum of operations");
    };
    section(out, "Requests");
    writeln!(
        out,
        "  defmodule Request do\n\
         \x20   @moduledoc ~S\"\"\"\n\
         \x20   One function per operation, each returning `{{:ok, request}}` or\n\
         \x20   `{{:error, message}}` after checking what it was given. The `!` form\n\
         \x20   beside each returns the request and raises instead.\n\
         \x20   \"\"\"\n"
    )?;
    let mut written = Vec::new();
    for variant in &request.variants {
        let name = &variant.name;
        if !is_function_name(name) {
            bail!(
                "the {name} operation cannot be spelled as an Elixir function; rename \
                 it on the wire or teach the generator to quote it"
            );
        }
        let mut documentation = variant.docs.clone();
        if let Some(structure) = data_struct(model, variant) {
            field_docs(&mut documentation, structure);
        }
        written.push(match &variant.payload {
            Payload::Unit => one_request(name, None, None, &documentation),
            Payload::Newtype(Shape::Ref(_)) | Payload::Struct(_) => {
                one_request(name, Some("data"), None, &documentation)
            }
            // An `Option` payload may be left out, as serde reads a missing
            // one as `None`; the argument defaults to leaving it out.
            Payload::Newtype(shape @ Shape::Optional(_)) => {
                let mut documentation = documentation.clone();
                documentation.push(String::new());
                documentation.push(format!(
                    "Takes one {} value, or none at all.",
                    shape.label()
                ));
                one_request(name, Some("value"), Some(":none"), &documentation)
            }
            Payload::Newtype(shape) => {
                let mut documentation = documentation.clone();
                documentation.push(String::new());
                documentation.push(format!("Takes one {} value.", shape.label()));
                one_request(name, Some("value"), None, &documentation)
            }
            Payload::Tuple(shapes) => {
                let labels: Vec<String> = shapes.iter().map(Shape::label).collect();
                let mut documentation = documentation.clone();
                documentation.push(String::new());
                documentation.push(format!("Takes a list of {}.", labels.join(", then ")));
                one_request(name, Some("values"), None, &documentation)
            }
        }?);
    }
    write!(out, "{}", written.join("\n"))?;
    writeln!(out, "  end")?;
    Ok(())
}

/// The struct a request's `data` is, when the payload is one.
fn data_struct<'model>(model: &'model Model, variant: &'model Variant) -> Option<&'model Struct> {
    match &variant.payload {
        Payload::Struct(structure) => Some(structure),
        Payload::Newtype(Shape::Ref(referenced)) => match model.get(referenced).map(|w| &w.body) {
            Some(Body::Struct(structure)) => Some(structure),
            _ => None,
        },
        _ => None,
    }
}

/// The pair of functions for one operation. `default` is what the argument
/// is when a caller leaves it out, for an operation whose payload may be.
fn one_request(
    name: &str,
    argument: Option<&str>,
    default: Option<&str>,
    documentation: &[String],
) -> Result<String> {
    let mut out = String::new();
    doc(&mut out, documentation, "    ")?;
    match argument {
        None => {
            writeln!(out, "    def {name}, do: {MODULE}.build({name:?})")?;
            writeln!(
                out,
                "\n    @doc \"`{name}/0`, raising on a request the server would refuse.\""
            )?;
            writeln!(out, "    def {name}!, do: {MODULE}.build!({name:?})")?;
        }
        Some(argument) => {
            let parameter = match default {
                Some(default) => format!("{argument} \\\\ {default}"),
                None => argument.to_owned(),
            };
            writeln!(
                out,
                "    def {name}({parameter}), do: {MODULE}.build({name:?}, {argument})"
            )?;
            writeln!(
                out,
                "\n    @doc \"`{name}/1`, raising on a request the server would refuse.\""
            )?;
            writeln!(
                out,
                "    def {name}!({parameter}), do: {MODULE}.build!({name:?}, {argument})"
            )?;
        }
    }
    Ok(out)
}

/// Wire spellings as a module apiece, so a client writes
/// `Response.answer()` rather than retyping `"answer"` at each call site,
/// and can turn one back into an atom to match on.
fn enums(out: &mut String, model: &Model) -> Result<()> {
    for wire in &model.types {
        let Body::Enum(enumeration) = &wire.body else {
            continue;
        };
        // `Request`'s variants are the operations, and that module is the
        // constructors. `operations/0` is where the bare list lives.
        if wire.name == "Request" {
            continue;
        }
        let spellings: Vec<String> = enumeration
            .variants
            .iter()
            .map(|variant| format!("    {{{}, {:?}}}", atom(&variant.name), variant.name))
            .collect();
        writeln!(out, "\ndefmodule {MODULE}.{} do", wire.name)?;
        let mut documentation = vec![format!("Wire spellings of `{}`.", wire.name)];
        if !wire.docs.is_empty() {
            documentation.push(String::new());
            documentation.extend(wire.docs.iter().cloned());
        }
        moduledoc_of(out, &documentation, "  ")?;
        writeln!(out, "\n  @spellings [\n{}\n  ]\n", spellings.join(",\n"))?;
        writeln!(
            out,
            "  @doc \"Every spelling as `{{atom, wire}}`, in declaration order.\"\n\
             \x20 def spellings, do: @spellings\n\n\
             \x20 @doc \"Every wire spelling, in declaration order.\"\n\
             \x20 def values, do: Enum.map(@spellings, &elem(&1, 1))\n\n\
             \x20 @doc \"The wire spelling of an atom, or nil.\"\n\
             \x20 def spelling(atom) do\n\
             \x20   case List.keyfind(@spellings, atom, 0) do\n\
             \x20     {{_atom, wire}} -> wire\n\
             \x20     nil -> nil\n\
             \x20   end\n\
             \x20 end\n\n\
             \x20 @doc \"The atom for a wire spelling: `{{:ok, atom}}` or `:error`.\"\n\
             \x20 def parse(wire) do\n\
             \x20   case List.keyfind(@spellings, wire, 1) do\n\
             \x20     {{atom, _wire}} -> {{:ok, atom}}\n\
             \x20     nil -> :error\n\
             \x20   end\n\
             \x20 end"
        )?;
        for variant in &enumeration.variants {
            // A variant whose spelling is not a function name is still in
            // `spellings/0`; only the shortcut is skipped, because a library
            // that refuses to describe an enum is worse than one that makes a
            // caller write the string.
            if !is_function_name(&variant.name) || RESERVED_BY_THE_MODULE.contains(&&*variant.name)
            {
                continue;
            }
            out.push('\n');
            doc(out, &variant.docs, "  ")?;
            writeln!(out, "  def {}, do: {:?}", variant.name, variant.name)?;
        }
        writeln!(out, "end")?;
    }
    Ok(())
}

/// Names each enum module already uses at arity zero.
const RESERVED_BY_THE_MODULE: [&str; 2] = ["spellings", "values"];

/// Whether a wire name can be written as an Elixir function name.
fn is_function_name(name: &str) -> bool {
    const KEYWORDS: [&str; 16] = [
        "after", "and", "catch", "def", "do", "else", "end", "false", "fn", "in", "nil", "not",
        "or", "rescue", "true", "when",
    ];
    !KEYWORDS.contains(&name)
        && !name.is_empty()
        && name.starts_with(|character: char| character.is_ascii_lowercase() || character == '_')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// A wire name as an Elixir atom, quoted where it has to be.
fn atom(name: &str) -> String {
    if is_function_name(name) {
        format!(":{name}")
    } else {
        format!(":{name:?}")
    }
}

/// The fields of a request's data, as a table in its doc.
fn field_docs(documentation: &mut Vec<String>, structure: &Struct) {
    if structure.fields.is_empty() {
        return;
    }
    let width = structure
        .fields
        .iter()
        .map(|field| field.name.len())
        .max()
        .unwrap_or(0);
    if !documentation.is_empty() {
        documentation.push(String::new());
    }
    documentation.push("Fields of `data`:".to_owned());
    documentation.push(String::new());
    for field in &structure.fields {
        documentation.push(format!(
            "  * `{:width$}`  {}{}",
            field.name,
            field.shape.label(),
            if field.required { "" } else { "  (optional)" },
        ));
    }
}

/// Rust doc comments as an `@doc` heredoc.
fn doc(out: &mut String, docs: &[String], indent: &str) -> Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    writeln!(out, "{indent}@doc ~S\"\"\"")?;
    heredoc(out, docs, indent)?;
    Ok(())
}

fn moduledoc_of(out: &mut String, docs: &[String], indent: &str) -> Result<()> {
    writeln!(out, "{indent}@moduledoc ~S\"\"\"")?;
    heredoc(out, docs, indent)
}

/// The body of a `~S` heredoc and its closing delimiter.
///
/// `~S` takes no escapes at all, which is what makes it the right sigil for
/// prose written somewhere else — a stray `#{` or backslash in a Rust doc
/// comment arrives as itself. The one thing it cannot carry is its own
/// delimiter, and a doc comment containing `"""` is rare enough that failing
/// is better than quietly mangling it.
fn heredoc(out: &mut String, docs: &[String], indent: &str) -> Result<()> {
    for line in docs {
        let line = super::prose(line);
        if line.contains("\"\"\"") {
            bail!("a doc comment contains `\"\"\"`, which an Elixir heredoc cannot carry: {line}");
        }
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "{indent}{line}")?;
        }
    }
    writeln!(out, "{indent}\"\"\"")?;
    Ok(())
}

/// Rust doc comments as ordinary `#` comments, for the places a doc attribute
/// cannot go — an entry in a map literal, say.
fn comment(out: &mut String, docs: &[String], indent: &str) {
    for line in docs {
        let line = super::prose(line);
        if line.is_empty() {
            out.push_str(&format!("{indent}#\n"));
        } else {
            out.push_str(&format!("{indent}# {line}\n"));
        }
    }
}

#[cfg(test)]
mod tests;
