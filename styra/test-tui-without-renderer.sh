#!/usr/bin/env bash
# Run separately from renderer tests: Cargo features are additive within a build.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."

dependency_graph=$(cargo tree -p tui --no-default-features --edges normal,build,dev --prefix none)
if grep -Eq '^(ratatui([ -]|$)|tui-markdown )' <<< "$dependency_graph"; then
    printf '%s\n' 'Application tests must not depend on a terminal renderer:' "$dependency_graph" >&2
    exit 1
fi

# Extra arguments go to the test harness, so they cannot enable Cargo features.
cargo test -p tui --lib --no-default-features -- "$@"
