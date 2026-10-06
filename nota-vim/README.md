# Nota for Vim

Review with the `nota` CLI from Vim or Neovim. Start or select a review,
read its notes and suggestion patches, and write notes without leaving the
editor. Review storage and validation belong to Nota.

Requires Vim 8.2+ or Neovim, Git, and `nota` on your `PATH`. Build the CLI
from this repository with `cargo build -p nota`, or install it with
`cargo install --path nota`.

## Install

Add `nota-vim/` to your plugin manager or Vim's runtime path:

```vim
set runtimepath+=/path/to/verka/nota-vim
```

To use a CLI outside `PATH`, set its executable path (without arguments):

```vim
let g:nota_executable = '/path/to/verka/target/debug/nota'
```

## Review

```vim
:NotaStart HEAD nota/my-review
:NotaNote Please explain the error handling.
:10,15NotaNote This needs a regression test.
:NotaNote
```

`NotaStart [revision] [branch]` defaults to `HEAD` and a generated branch name.
It opens the review and remembers its branch for that worktree. Starting a
review and adding notes leave your checkout and index untouched.

`NotaShow [branch]` opens an existing review and selects it. In the review
buffer, press `Enter` on the subject or an entry to read its full commit
message and patch, `r` to refresh, `n` to compose a note, or `q` to close.
Entry buffers also use `q` to close. Use `r` after recording notes or
committing suggestions.

`NotaBranch {branch}` selects an existing review without opening it.
`NotaBranch` clears the selection. With no selection, commands use the
checked-out branch, as the CLI does. Selections are independent per worktree
and last until Vim exits. Commands find the worktree from the current file,
or from Vim's working directory in a buffer without a file. Review, entry,
and note buffers carry their own repository and branch context.

`NotaNote {text}` submits the rest of the command line as a note. With no
text, it opens an editable Markdown draft: `:write` submits it and closes the
draft; `:bdelete!` discards it. Failed submissions preserve the draft. Hidden
drafts can be recovered with `:buffers!` and `:buffer {number}`. Each draft
keeps the branch it was opened for, even if you select another review later.

A range such as `:'<,'>NotaNote` appends `Source: path:10-15` to the submitted
note. Paths are relative to the worktree and line numbers refer to the buffer
at the time you run the command; this is prose context, not a structured
location tracked by Nota. Plain `NotaNote` writes a general note.

## Suggestions

Nota represents suggestions as ordinary Git commits on the review branch.
Create a separate worktree, open the files there in Vim, edit, and commit:

```sh
git worktree add ../my-review nota/my-review
git -C ../my-review add src/example.rs
git -C ../my-review commit -m 'Handle the missing value explicitly'
```

The commit appears as a suggestion in `NotaShow`. Accepted suggestions are
cherry-picked individually onto the destination branch. Published reviews
are append-only; do not amend, rebase, or force-push their commits. See
[Nota's prototype specification](../nota/PROTOTYPE_V1.md).

## Test

The integration test uses the real CLI and disposable Git worktrees:

```sh
cargo build -p nota
vim -Nu NONE -n -es -S nota-vim/tests/integration.vim
nvim --headless -u NONE -n -S nota-vim/tests/integration.vim
```

Set `NOTA_TEST_EXECUTABLE` to test a different CLI binary.
