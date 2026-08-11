//! Recording and verifying which project repository a store describes.

use super::*;

/// Record which project repository this store describes, keyed by the
/// project's root commit (`pairing.toml` in the store, committed to the
/// workbench repository like any other store change). Idempotent when the
/// recorded root already matches. A mismatch is the error this exists to
/// catch — the wrong project sitting in the workbench, or a rewritten
/// history — and needs `force` to overwrite deliberately.
///
/// Two purely informational fields ride along for human readers, never
/// checked by anything: `name`, given by the caller, and the project's
/// `origin` remote URL, observed here. On a same-root re-pair they are
/// refreshed (a given name wins; a currently-present remote wins) without
/// touching the identity or its timestamp.
pub fn pair(store: &Store, vcs: &dyn Vcs, name: Option<String>, force: bool) -> Result<Pairing> {
    let mutation = store.mutation_lock(vcs)?;
    let Some(root) = vcs.root_commit()? else {
        bail!("the project repository has no commits yet — nothing to pair to");
    };
    let remote = vcs.remote_url()?;
    if let Some(existing) = Pairing::load(store.root())? {
        if existing.root_commit == root {
            let updated = Pairing {
                name: name.or_else(|| existing.name.clone()),
                remote: remote.or_else(|| existing.remote.clone()),
                ..existing.clone()
            };
            if updated.name == existing.name && updated.remote == existing.remote {
                return Ok(existing);
            }
            updated.save(store.root())?;
            mutation.commit(vcs, "linka: pair project (update info)")?;
            return Ok(updated);
        }
        if !force {
            bail!(
                "store is paired to project root {} but the project's root is {} — \
                 wrong project in the workbench, or a rewritten history \
                 (re-pair with --force if this is intentional)",
                short(&existing.root_commit),
                short(&root)
            );
        }
    }
    let pairing = Pairing {
        schema: 1,
        root_commit: root,
        paired_at: now_millis(),
        name,
        remote,
    };
    pairing.save(store.root())?;
    mutation.commit(vcs, "linka: pair project")?;
    Ok(pairing)
}

/// Verify the store↔project pairing. Read-only and manual — nothing calls it
/// implicitly. Returns the recorded pairing (`None` means the store is not
/// paired, which is a notice, not a problem — stores predating pairing
/// exist) and the list of problems found. Only the root commit is checked;
/// the pairing's name and remote are information for the caller to display.
///
/// The default check is one comparison: the project's actual root commit
/// against the recorded one. With `deep`, every hash the store points at —
/// each result's output commit and every consumed output pin — is also
/// checked to exist in the project repository, catching partial history
/// rewrites that leave the root intact but orphan recorded outputs.
pub fn verify_pairing(
    store: &Store,
    vcs: &dyn Vcs,
    deep: bool,
) -> Result<(Option<Pairing>, Vec<String>)> {
    let Some(pairing) = Pairing::load(store.root())? else {
        return Ok((None, Vec::new()));
    };
    let mut problems = Vec::new();
    match vcs.root_commit()? {
        None => problems.push(format!(
            "project repository has no commits, but the store is paired to root {}",
            short(&pairing.root_commit)
        )),
        Some(actual) if actual != pairing.root_commit => problems.push(format!(
            "project root commit is {} but the store is paired to {} — \
             wrong project in the workbench, or a rewritten history \
             (`linka pair --force` re-pairs deliberately)",
            short(&actual),
            short(&pairing.root_commit)
        )),
        Some(_) => {}
    }
    if deep {
        for id in store.list_ids()? {
            let Some((result, _)) = store.read_result(&id)? else {
                continue;
            };
            if let Some(output) = &result.output {
                if !vcs.commit_exists(&output.id)? {
                    problems.push(format!(
                        "{id}: output commit {} does not exist in the project repository",
                        short(&output.id)
                    ));
                }
            }
            for consumed in &result.consumed {
                if let Some(output) = &consumed.output {
                    if !vcs.commit_exists(&output.id)? {
                        problems.push(format!(
                            "{id}: built-against output {} (of {}) does not exist in the project repository",
                            short(&output.id),
                            consumed.id
                        ));
                    }
                }
            }
        }
    }
    Ok((Some(pairing), problems))
}
