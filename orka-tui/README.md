# orka-tui

A terminal control panel for an Orka workbench. It exposes Orka's ready queue,
attempts and their evidence, candidates and patches, active reviews, managed
review worktrees, recovery, publication, and evidence audit.

Run it from anywhere inside a Linka/Orka workbench:

```text
cargo run --manifest-path /path/to/verka/orka-tui/Cargo.toml
```

Or pass the workbench explicitly:

```text
cargo run --manifest-path orka-tui/Cargo.toml -- --workbench /path/to/workbench
```

Use `←`/`→` or `1`–`7` to change views, `j`/`k` to select, `Enter` to inspect,
and `a` for the context-sensitive action menu. Press `?` for the full key map.
Long-running attempts and recovery run on a worker thread; lifecycle progress
is shown in the status line. Refresh/load errors are retained in the Errors
view and action errors open as visible dialogs.
