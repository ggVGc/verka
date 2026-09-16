//! The wire vocabulary, read back out of the Rust definitions that declare it.
//!
//! Nothing here knows about Lua. It answers one question — what shapes cross
//! the socket, and what does serde make of them — from the same source text
//! serde's derives are expanded from, so a model can only ever describe the
//! protocol as it actually is.
//!
//! The reading is deliberately narrow. Only the serde attributes that change
//! the bytes are honoured (`rename`, `rename_all`, `tag`, `content`,
//! `untagged`, `default`, `skip`, `deny_unknown_fields`); anything else is
//! ignored, and anything that cannot be understood at all — a type the
//! generator has no mapping for, a name no source defines — is an error rather
//! than a guess, because a binding generated from a guess is worse than no
//! binding.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, HashSet, VecDeque};
use syn::{Attribute, Expr, ExprLit, Fields, GenericArgument, Item, Lit, Meta, PathArguments};

/// Every wire type reachable from the protocol's roots, roots first.
#[derive(Clone, Debug)]
pub struct Model {
    pub types: Vec<WireType>,
}

#[derive(Clone, Debug)]
pub struct WireType {
    /// The Rust name, which is also how the generated library refers to it.
    pub name: String,
    pub docs: Vec<String>,
    pub body: Body,
}

#[derive(Clone, Debug)]
pub enum Body {
    Struct(Struct),
    Enum(Enum),
}

#[derive(Clone, Debug)]
pub struct Struct {
    pub fields: Vec<Field>,
    pub deny_unknown_fields: bool,
}

#[derive(Clone, Debug)]
pub struct Field {
    /// The wire name, after `rename`/`rename_all`.
    pub name: String,
    pub docs: Vec<String>,
    pub shape: Shape,
    /// Whether the sender must supply it: true unless serde can fill it in.
    pub required: bool,
}

#[derive(Clone, Debug)]
pub struct Enum {
    pub tagging: Tagging,
    pub variants: Vec<Variant>,
    /// Applies to the fields of struct variants.
    pub deny_unknown_fields: bool,
}

/// How serde spells a variant on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tagging {
    /// `{"variant": payload}`, or the bare name for a unit variant.
    External,
    /// The payload's own fields, with the variant name under `tag`.
    Internal { tag: String },
    /// `{tag: "variant", content: payload}`.
    Adjacent { tag: String, content: String },
    /// Whatever the payload serializes to, with nothing naming the variant.
    Untagged,
}

#[derive(Clone, Debug)]
pub struct Variant {
    pub name: String,
    /// The Rust identifier, for naming a constant after it.
    pub ident: String,
    pub docs: Vec<String>,
    pub payload: Payload,
}

#[derive(Clone, Debug)]
pub enum Payload {
    Unit,
    Newtype(Shape),
    /// Several unnamed values, which serde writes as a JSON array.
    Tuple(Vec<Shape>),
    Struct(Struct),
}

/// What one value is, as far as the wire is concerned.
#[derive(Clone, Debug)]
pub enum Shape {
    Bool,
    Integer,
    Number,
    Text,
    /// A filesystem path, which is a string that means something particular.
    Path,
    /// Arbitrary JSON.
    Json,
    List(Box<Shape>),
    Map(Box<Shape>),
    Optional(Box<Shape>),
    Ref(String),
}

impl Shape {
    /// The names this shape refers to, for walking the type graph.
    fn references(&self, into: &mut Vec<String>) {
        match self {
            Shape::Ref(name) => into.push(name.clone()),
            Shape::List(inner) | Shape::Map(inner) | Shape::Optional(inner) => {
                inner.references(into)
            }
            _ => {}
        }
    }

    /// How the shape reads in a generated doc comment.
    pub fn label(&self) -> String {
        match self {
            Shape::Bool => "boolean".into(),
            Shape::Integer | Shape::Number => "number".into(),
            Shape::Text => "string".into(),
            Shape::Path => "path".into(),
            Shape::Json => "any".into(),
            Shape::List(inner) => format!("{}[]", inner.label()),
            Shape::Map(inner) => format!("table<string, {}>", inner.label()),
            Shape::Optional(inner) => format!("{}|null", inner.label()),
            Shape::Ref(name) => name.clone(),
        }
    }
}

impl WireType {
    fn references(&self) -> Vec<String> {
        let mut names = Vec::new();
        match &self.body {
            Body::Struct(structure) => {
                for field in &structure.fields {
                    field.shape.references(&mut names);
                }
            }
            Body::Enum(enumeration) => {
                for variant in &enumeration.variants {
                    match &variant.payload {
                        Payload::Unit => {}
                        Payload::Newtype(shape) => shape.references(&mut names),
                        Payload::Tuple(shapes) => {
                            for shape in shapes {
                                shape.references(&mut names);
                            }
                        }
                        Payload::Struct(structure) => {
                            for field in &structure.fields {
                                field.shape.references(&mut names);
                            }
                        }
                    }
                }
            }
        }
        names
    }

    /// Whether every variant is a bare name — the enums a client holds as
    /// plain strings.
    pub fn is_plain_enum(&self) -> bool {
        match &self.body {
            Body::Enum(enumeration) => enumeration
                .variants
                .iter()
                .all(|variant| matches!(variant.payload, Payload::Unit)),
            Body::Struct(_) => false,
        }
    }
}

impl Model {
    /// Read `roots` and everything they reach out of the given Rust sources.
    ///
    /// Each source is `(label, text)`; the label only ever appears in errors.
    pub fn build(sources: &[(&str, &str)], roots: &[&str]) -> Result<Self> {
        let index = Index::read(sources)?;
        let mut types = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, Option<String>)> =
            roots.iter().map(|name| ((*name).into(), None)).collect();
        while let Some((name, referrer)) = queue.pop_front() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let item = index.get(&name, referrer.as_deref())?;
            let wire = read_item(&name, item)?;
            for reference in wire.references() {
                queue.push_back((reference, Some(name.clone())));
            }
            types.push(wire);
        }
        Ok(Self { types })
    }

    pub fn get(&self, name: &str) -> Option<&WireType> {
        self.types.iter().find(|wire| wire.name == name)
    }
}

/// Every struct and enum the given sources define, by name.
struct Index {
    items: BTreeMap<String, Vec<(String, Item)>>,
}

impl Index {
    fn read(sources: &[(&str, &str)]) -> Result<Self> {
        let mut items: BTreeMap<String, Vec<(String, Item)>> = BTreeMap::new();
        for (label, text) in sources {
            let file = syn::parse_file(text)
                .with_context(|| format!("{label} could not be parsed as Rust"))?;
            collect(&file.items, label, &mut items);
        }
        Ok(Self { items })
    }

    fn get(&self, name: &str, referrer: Option<&str>) -> Result<&Item> {
        let named = || match referrer {
            Some(referrer) => format!("{name}, referred to by {referrer}"),
            None => name.to_owned(),
        };
        let found = self
            .items
            .get(name)
            .ok_or_else(|| anyhow!("no indexed source defines {}", named()))?;
        match found.as_slice() {
            [(_, item)] => Ok(item),
            [] => bail!("no indexed source defines {}", named()),
            many => bail!(
                "{} is defined in more than one indexed source ({}); the generator \
                 cannot tell which one the protocol means",
                named(),
                many.iter()
                    .map(|(label, _)| label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// Index the structs and enums in `items`, descending into inline modules.
/// Definitions inside function bodies and impl blocks are deliberately out of
/// reach: they are private helpers, never part of anyone's wire surface.
fn collect(items: &[Item], label: &str, into: &mut BTreeMap<String, Vec<(String, Item)>>) {
    for item in items {
        match item {
            Item::Struct(structure) => into
                .entry(structure.ident.to_string())
                .or_default()
                .push((label.to_owned(), item.clone())),
            Item::Enum(enumeration) => into
                .entry(enumeration.ident.to_string())
                .or_default()
                .push((label.to_owned(), item.clone())),
            Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    collect(nested, label, into);
                }
            }
            _ => {}
        }
    }
}

fn read_item(name: &str, item: &Item) -> Result<WireType> {
    match item {
        Item::Struct(structure) => {
            let attrs = serde_attrs(&structure.attrs)
                .with_context(|| format!("reading the serde attributes of {name}"))?;
            let fields = read_fields(name, &structure.fields, &attrs)?;
            Ok(WireType {
                name: name.to_owned(),
                docs: docs(&structure.attrs),
                body: Body::Struct(Struct {
                    fields,
                    deny_unknown_fields: attrs.deny_unknown_fields,
                }),
            })
        }
        Item::Enum(enumeration) => {
            let attrs = serde_attrs(&enumeration.attrs)
                .with_context(|| format!("reading the serde attributes of {name}"))?;
            let tagging = match (&attrs.tag, &attrs.content, attrs.untagged) {
                (_, _, true) => Tagging::Untagged,
                (Some(tag), Some(content), _) => Tagging::Adjacent {
                    tag: tag.clone(),
                    content: content.clone(),
                },
                (Some(tag), None, _) => Tagging::Internal { tag: tag.clone() },
                (None, _, _) => Tagging::External,
            };
            let mut variants = Vec::new();
            for variant in &enumeration.variants {
                let variant_attrs = serde_attrs(&variant.attrs).with_context(|| {
                    format!("reading the serde attributes of {name}::{}", variant.ident)
                })?;
                if variant_attrs.skip {
                    continue;
                }
                let ident = variant.ident.to_string();
                let wire = variant_attrs
                    .rename
                    .clone()
                    .unwrap_or_else(|| rename(&ident, attrs.rename_all.as_deref()));
                let payload = match &variant.fields {
                    Fields::Unit => Payload::Unit,
                    Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
                        let only = unnamed.unnamed.first().expect("one field");
                        Payload::Newtype(
                            shape(&only.ty).with_context(|| {
                                format!("reading the payload of {name}::{ident}")
                            })?,
                        )
                    }
                    Fields::Unnamed(unnamed) => {
                        let mut shapes = Vec::new();
                        for (position, field) in unnamed.unnamed.iter().enumerate() {
                            shapes.push(shape(&field.ty).with_context(|| {
                                format!("reading element {position} of {name}::{ident}")
                            })?);
                        }
                        Payload::Tuple(shapes)
                    }
                    fields => Payload::Struct(Struct {
                        fields: read_fields(&format!("{name}::{ident}"), fields, &variant_attrs)?,
                        deny_unknown_fields: attrs.deny_unknown_fields
                            || variant_attrs.deny_unknown_fields,
                    }),
                };
                variants.push(Variant {
                    name: wire,
                    ident,
                    docs: docs(&variant.attrs),
                    payload,
                });
            }
            Ok(WireType {
                name: name.to_owned(),
                docs: docs(&enumeration.attrs),
                body: Body::Enum(Enum {
                    tagging,
                    variants,
                    deny_unknown_fields: attrs.deny_unknown_fields,
                }),
            })
        }
        _ => bail!("{name} is neither a struct nor an enum"),
    }
}

fn read_fields(owner: &str, fields: &Fields, container: &SerdeAttrs) -> Result<Vec<Field>> {
    let named = match fields {
        Fields::Named(named) => &named.named,
        Fields::Unit => return Ok(Vec::new()),
        Fields::Unnamed(_) => bail!("{owner} has unnamed fields, which have no wire names"),
    };
    let mut read = Vec::new();
    for field in named {
        let ident = field
            .ident
            .as_ref()
            .expect("named fields have identifiers")
            .to_string();
        let attrs = serde_attrs(&field.attrs)
            .with_context(|| format!("reading the serde attributes of {owner}.{ident}"))?;
        if attrs.skip {
            continue;
        }
        if attrs.flatten {
            bail!(
                "{owner}.{ident} is flattened; the generator has no way to describe \
                 a field whose own fields stand in for it"
            );
        }
        let shape =
            shape(&field.ty).with_context(|| format!("reading the type of {owner}.{ident}"))?;
        read.push(Field {
            name: attrs
                .rename
                .clone()
                .unwrap_or_else(|| rename(&ident, container.rename_all.as_deref())),
            docs: docs(&field.attrs),
            shape,
            // Anything serde can supply itself is optional to a sender; what it
            // cannot is the sender's to provide, nullable or not.
            required: !attrs.default && !container.default,
        });
    }
    Ok(read)
}

/// Map one Rust type onto what it becomes in JSON.
fn shape(ty: &syn::Type) -> Result<Shape> {
    let syn::Type::Path(path) = ty else {
        bail!("only plain named types can cross the wire");
    };
    let segment = path
        .path
        .segments
        .last()
        .ok_or_else(|| anyhow!("a type path with no segments"))?;
    let name = segment.ident.to_string();
    let mut arguments = Vec::new();
    if let PathArguments::AngleBracketed(bracketed) = &segment.arguments {
        for argument in &bracketed.args {
            if let GenericArgument::Type(inner) = argument {
                arguments.push(inner);
            }
        }
    }
    let argument = |position: usize| -> Result<Shape> {
        let inner = arguments
            .get(position)
            .ok_or_else(|| anyhow!("{name} is missing a type argument"))?;
        shape(inner)
    };
    Ok(match name.as_str() {
        "bool" => Shape::Bool,
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize" => {
            Shape::Integer
        }
        "f32" | "f64" => Shape::Number,
        "String" | "str" => Shape::Text,
        "PathBuf" | "Path" => Shape::Path,
        "Value" => Shape::Json,
        "Option" => Shape::Optional(Box::new(argument(0)?)),
        "Vec" | "VecDeque" => Shape::List(Box::new(argument(0)?)),
        "HashMap" | "BTreeMap" => Shape::Map(Box::new(argument(1)?)),
        _ if arguments.is_empty() => Shape::Ref(name),
        _ => bail!("{name} is generic, and the generator has no mapping for it"),
    })
}

/// The serde attributes that change what crosses the wire. Everything else a
/// `#[serde(...)]` can say is about Rust, not about the bytes.
#[derive(Default)]
struct SerdeAttrs {
    rename: Option<String>,
    rename_all: Option<String>,
    tag: Option<String>,
    content: Option<String>,
    untagged: bool,
    deny_unknown_fields: bool,
    default: bool,
    skip: bool,
    flatten: bool,
}

fn serde_attrs(attrs: &[Attribute]) -> Result<SerdeAttrs> {
    let mut read = SerdeAttrs::default();
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            let value = if meta.input.peek(syn::Token![=]) {
                Some(meta.value()?.parse::<syn::LitStr>()?.value())
            } else {
                None
            };
            match (key.as_str(), value) {
                ("rename", Some(value)) => read.rename = Some(value),
                ("rename_all", Some(value)) => read.rename_all = Some(value),
                ("tag", Some(value)) => read.tag = Some(value),
                ("content", Some(value)) => read.content = Some(value),
                ("untagged", None) => read.untagged = true,
                ("deny_unknown_fields", None) => read.deny_unknown_fields = true,
                ("default", _) => read.default = true,
                ("skip" | "skip_serializing" | "skip_deserializing", None) => read.skip = true,
                ("flatten", None) => read.flatten = true,
                // `alias`, `skip_serializing_if`, `with`, `borrow`, … say
                // nothing a client has to be told.
                _ => {}
            }
            Ok(())
        })?;
    }
    Ok(read)
}

fn docs(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(pair) => match &pair.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(text),
                    ..
                }) => Some(text.value().trim().to_owned()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Apply a `rename_all` style, matching serde's own case conversion.
fn rename(ident: &str, style: Option<&str>) -> String {
    let Some(style) = style else {
        return ident.to_owned();
    };
    match style {
        "lowercase" => ident.to_lowercase(),
        "UPPERCASE" => ident.to_uppercase(),
        "PascalCase" => upper_camel(ident),
        "camelCase" => {
            let pascal = upper_camel(ident);
            let mut chars = pascal.chars();
            match chars.next() {
                Some(first) => first.to_lowercase().chain(chars).collect(),
                None => pascal,
            }
        }
        "snake_case" => separated(ident, '_'),
        "SCREAMING_SNAKE_CASE" => separated(ident, '_').to_uppercase(),
        "kebab-case" => separated(ident, '-'),
        "SCREAMING-KEBAB-CASE" => separated(ident, '-').to_uppercase(),
        other => panic!("unhandled serde rename_all style {other:?}"),
    }
}

/// serde's snake/kebab conversion: a separator before every capital after the
/// first, everything lowercased. `XHigh` is `x_high`, as serde spells it.
fn separated(ident: &str, separator: char) -> String {
    let mut out = String::new();
    for (position, character) in ident.char_indices() {
        if character.is_uppercase() && position != 0 {
            out.push(separator);
        }
        out.extend(character.to_lowercase());
    }
    out
}

/// A Rust variant identifier is already PascalCase; only underscores in one
/// need folding out.
fn upper_camel(ident: &str) -> String {
    let mut out = String::new();
    let mut capitalize = true;
    for character in ident.chars() {
        if character == '_' {
            capitalize = true;
            continue;
        }
        if capitalize {
            out.extend(character.to_uppercase());
            capitalize = false;
        } else {
            out.push(character);
        }
    }
    out
}

/// `SCREAMING_SNAKE_CASE`, for naming a Lua constant after a variant.
pub fn shouted(ident: &str) -> String {
    separated(ident, '_').to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_all_matches_serdes_own_case_conversion() {
        assert_eq!(rename("XHigh", Some("lowercase")), "xhigh");
        assert_eq!(rename("CodexExec", Some("kebab-case")), "codex-exec");
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
}
