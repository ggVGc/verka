# Linka TUI

`linka-tui` is an interactive terminal frontend for the `linka` library. It
does not parse CLI output or duplicate Linka's graph rules: every query and
mutation goes through Linka's public Rust API.

Run it from a Linka workbench:

```sh
cargo run --manifest-path linka-tui/Cargo.toml -- --store /path/to/workbench/.linka
```

Create a workbench and open it in one step:

```sh
cargo run --manifest-path linka-tui/Cargo.toml -- --store ./workbench/.linka --init
```

The top tabs list all nodes, candidates, verification nodes, ready work, stale
work, blocked work, and evaluation errors. The right-hand association pane
links dependencies, lineage, dependents, candidates, source nodes, and
verifications; press `Tab` and `Enter` to follow one, then `b` to return.

Each node row starts with a sigil for its kind: `■` (blue) for a work node and
`◆` (magenta) for a verification node, so mixed collections stay readable. The
details pane spells the kind out, and `?` shows the legend.

Press `A` to browse the selected node's attachments (or, in the Candidates
collection, its source node's). The left pane lists namespace/key with size and
media type; the right pane shows the selected payload as text, or as a hex dump
when the bytes are not UTF-8. `j`/`k` select, `J`/`K` scroll, `Esc` closes.

Press `a` to open the action palette. It includes graph creation and editing,
completion and failure, responses, verification results, candidate
registration/decisions/publication, attachments, context observations,
history, origin and settlement queries, integrity checks, and project pairing.
Press `?` in the application for the complete key reference.


This crate is also a library: `linka_tui::app::App` plus `linka_tui::ui::draw_in`
let a host application embed the interface inside an area of its own frame.
[orka-tui](../orka-tui) does this behind its `L` shortcut.
