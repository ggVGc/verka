# Styra Live

A minimal Phoenix LiveView client for the local Styra server. It uses the
hand-written socket transport in `../styra-elixir` and the generated Elixir
vocabulary in `../styra/protocol/elixir`; request shapes are not duplicated in
the web application.

The dashboard:

- connects to a configurable Styra Unix socket;
- refreshes and selects live interactions in a dropdown above the log;
- streams decoded interaction updates with cursor-based polling;
- records and transcribes microphone audio into the message editor for review;
- composes messages in a centered modal, preserving drafts when dismissed;
- sends messages with optional `text`, `lines`, `files`, or `json` answer
  contracts; and
- interrupts or stops the selected interaction.

## Run

Start `styra-server`, then:

```sh
cd styra-web
mix setup
mix phx.server
```

Open <http://localhost:4000>. The socket is deployment configuration and is
never accepted from the browser. It defaults to
`$XDG_RUNTIME_DIR/styra/styra.sock`; set `STYRA_SOCKET_PATH` before starting the
application to override it.

Voice messages require microphone permission in the browser and the same local
Whisper model used by Styra's TUI. Fetch it once with
`styra-transcribe --download-model` before recording. The transcript is inserted
at the editor's current cursor position; review or edit it, then press **Send**.

The console follows the TUI's dark palette and compact log layout. Press **l**
to toggle live interactions, use **↑/↓** (or **j/k**) to move through them, and
press **Enter** to select. Press **i** to compose a message. In the message box,
**Enter** sends, **Shift+Enter** inserts a newline, and **Esc** closes the box
while keeping the draft. The log follows new events while scrolled to the end;
scroll up to read earlier output without being pulled back down.

## Verify

```sh
mix precommit
```

The LiveView test injects a fake exchange into `Styra.Client`, exercising the
real generated request constructors without requiring a running Styra daemon.
