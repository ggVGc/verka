//! Write the generated Lua client library.
//!
//! With no arguments it refreshes the copy checked in under the crate, which
//! is what `styra-protocol`'s staleness test compares against. `--stdout`
//! prints it instead, and a path writes it wherever a Lua project keeps its
//! dependencies.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let library = styra_protocol::lua::library().context("generating the Lua library")?;
    let mut arguments = std::env::args().skip(1);
    let destination = match arguments.next().as_deref() {
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(styra_protocol::lua::GENERATED_PATH),
        Some("--stdout" | "-") => {
            print!("{library}");
            return Ok(());
        }
        Some("--help" | "-h") => {
            println!(
                "usage: styra-protocol-lua [PATH|--stdout]\n\n\
                 With no arguments, rewrites {}.",
                styra_protocol::lua::GENERATED_PATH
            );
            return Ok(());
        }
        Some(path) => PathBuf::from(path),
    };
    if arguments.next().is_some() {
        bail!("styra-protocol-lua takes at most one destination");
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
