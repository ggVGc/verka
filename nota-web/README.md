# Nota Web

A Phoenix LiveView viewer for [nota](../nota) review branches.

It points at a Git repository, lists the review branches `nota list` discovers,
and renders the selected review: each note as a comment card and each
suggestion as a commit with its changed files and unified diff. Notes that are
anchored to file lines show the pinned source snippet.

## How it reads a review

- The `nota` executable provides the review structure and validation through
  its `list --json` and `show --json` output.
- `git` provides commit metadata, diffs, and the file lines a note points at.
  Nothing is ever checked out; every command runs with `git -C <repo>`.
- Suggestion diffs load the first time a suggestion is expanded. Sources for
  line-anchored notes load with the review.

The repository and branch live in the URL (`/?repository=...&branch=...`) so a
view can be shared or reloaded.

## Run

Build the `nota` binary once from `../nota`, then:

```sh
mix setup
mix phx.server
```

Open <http://localhost:4000>. Point the viewer at a repository with the field
at the top of the page.

Configuration is read from the environment:

- `NOTA_REPOSITORY` — the repository to open by default; defaults to the
  current working directory.
- `NOTA_BIN` — the `nota` executable to run; defaults to `nota` on `PATH`.
- `PORT` — the HTTP port; defaults to `4000`.

## Verify

```sh
mix precommit
```

The LiveView test substitutes an in-memory `NotaWeb.FakeNota`, so the suite
does not need a built `nota` binary or a real repository. The diff parser and
JSON decoding are tested directly.
