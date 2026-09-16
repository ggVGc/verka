//! Write a generated client library.
//!
//! The language is named first, and with nothing after it the checked-in copy
//! for that language is refreshed — which is what the staleness test compares
//! against. `--stdout` prints it instead, and a path writes it wherever the
//! project on the other end keeps its dependencies.
//!
//!   styra-codegen lua                 # rewrite the checked-in copy
//!   styra-codegen lua --stdout        # print it
//!   styra-codegen lua ../elsewhere.lua
//!
//! With no arguments at all it lists what it can generate, since that is the
//! one question a caller who mistyped the language actually has.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use styra_protocol::codegen;

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let language = match arguments.next().as_deref() {
        None => {
            println!(
                "usage: styra-codegen <{}> [PATH|--stdout]",
                codegen::names().join("|")
            );
            return Ok(());
        }
        Some("--help" | "-h") => {
            println!(
                "usage: styra-codegen <LANGUAGE> [PATH|--stdout]\n\n\
                 Generates a client library from the Styra protocol's own type\n\
                 definitions. With no destination, rewrites the copy checked in\n\
                 for that language.\n\n\
                 Languages: {}",
                codegen::names().join(", ")
            );
            return Ok(());
        }
        Some(name) => codegen::language(name)?,
    };

    let library = codegen::generate(language)
        .with_context(|| format!("generating the {} library", language.name()))?;
    let destination = match arguments.next().as_deref() {
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(language.generated_path()),
        Some("--stdout" | "-") => {
            print!("{library}");
            return Ok(());
        }
        Some(path) => PathBuf::from(path),
    };
    if arguments.next().is_some() {
        bail!("styra-codegen takes at most one destination");
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&destination, &library)
        .with_context(|| format!("writing {}", destination.display()))?;
    eprintln!("wrote {}", destination.display());
    Ok(())
}
