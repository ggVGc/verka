# Styra Live

A minimal Phoenix LiveView client for the local Styra server. It uses the
hand-written socket transport in `../styra-elixir` and the generated Elixir
vocabulary in `../styra/protocol/elixir`; request shapes are not duplicated in
the web application.

The dashboard:

- connects to a configurable Styra Unix socket;
- refreshes and selects live interactions;
- streams decoded interaction updates with cursor-based polling;
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

## Verify

```sh
mix precommit
```

The LiveView test injects a fake exchange into `Styra.Client`, exercising the
real generated request constructors without requiring a running Styra daemon.
