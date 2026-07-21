# Orka Web

Orka Web is a local, live interface for an Orka workbench. It shows Orka's
machine-ready queue, durable attempts and transcripts, candidate integration,
and active reviews in the context of the Linka graph Orka is orchestrating.

Run it from anywhere below a workbench:

```text
cargo run --manifest-path /path/to/verka/orka-web/Cargo.toml
```

Or point it at a workbench explicitly:

```text
orka-web --workbench /path/to/workbench --addr 127.0.0.1:7710
```

`ORKA_WORKBENCH` can provide the workbench path. The server binds to localhost
by default and serves a self-contained page with no external assets. It reads
Orka and Linka through their public Rust APIs. The only write currently exposed
by the page is responding to a ready human-assigned node.

The work-log view consumes Orka's normalized `WorkLogBlock` representation,
not model-specific JSONL. Fenced Markdown code is shown as a code block and is
syntax-highlighted using its declared language; legacy plain-output attempts
remain available through the same format.
