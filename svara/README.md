# Svara

Svara is a small Neovim client for an existing Styra session. Its Lua library
sends one `send_message` request over the `styra-server` Unix socket; both the
command-line program and Neovim command use that same function.

## Neovim

Add this directory to Neovim's runtime path with your plugin manager, then run:

```vim
:SvaraSend styra-7 Review this buffer
```

The first argument is the ID of an existing live session. Everything after it
is sent as the message.

The shared Lua interface is also available directly:

```lua
local sent, err = require("svara").send_message("styra-7", "Review this buffer")
```

## CLI

The CLI is a Lua script executed by headless Neovim, so it has the same libuv
Unix-socket and JSON implementation as the plugin and needs no extra Lua
packages:

```sh
./bin/svara styra-7 "Review this buffer"
```

By default Svara connects to `$XDG_RUNTIME_DIR/styra/styra.sock`, matching
`styra-server`. Set `STYRA_SOCKET` to use another socket. Neovim 0.10 or newer
is required.

## Test

```sh
nvim --headless -u NONE -l tests/core_spec.lua
```
