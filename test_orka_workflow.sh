#!/usr/bin/env bash

set -euo pipefail

usage() {
    echo "usage: $0 DIRECTORY" >&2
    echo "DIRECTORY must be empty or not yet exist." >&2
}

if [[ $# -ne 1 ]]; then
    usage
    exit 2
fi

for command in cargo git bwrap codex; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "error: required command not found: $command" >&2
        exit 1
    fi
done

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
target_dir="$1"

mkdir -p -- "$target_dir"
target_dir="$(cd -- "$target_dir" && pwd -P)"

if [[ -n "$(find "$target_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    echo "error: target directory is not empty: $target_dir" >&2
    exit 1
fi

linka_bin="${LINKA_BIN:-$repo_root/linka/target/debug/linka}"
orka_bin="${ORKA_BIN:-$repo_root/orka/target/debug/orka}"

if [[ "${SKIP_BUILD:-0}" != 1 ]]; then
    echo "==> Building Linka and Orka"
    cargo build --quiet --manifest-path "$repo_root/linka/Cargo.toml"
    cargo build --quiet --manifest-path "$repo_root/orka/Cargo.toml"
fi

for binary in "$linka_bin" "$orka_bin"; do
    if [[ ! -x "$binary" ]]; then
        echo "error: executable not found: $binary" >&2
        exit 1
    fi
done

on_exit() {
    status=$?
    if [[ $status -ne 0 ]]; then
        echo "FAILED: workflow fixture retained at $target_dir" >&2
    fi
}
trap on_exit EXIT

echo "==> Initializing test repositories on main"
git -C "$target_dir" init -q -b main
git -C "$target_dir" config user.name "Orka Workflow Test"
git -C "$target_dir" config user.email "orka-test@example.invalid"

(
    cd "$target_dir"
    GIT_AUTHOR_NAME="Orka Workflow Test" \
        GIT_AUTHOR_EMAIL="orka-test@example.invalid" \
        GIT_COMMITTER_NAME="Orka Workflow Test" \
        GIT_COMMITTER_EMAIL="orka-test@example.invalid" \
        "$orka_bin" init --create-project
)
git -C "$target_dir/project" config user.name "Orka Workflow Test"
git -C "$target_dir/project" config user.email "orka-test@example.invalid"

echo "==> Creating one machine work node"
node_id="$(
    cd "$target_dir"
    "$linka_bin" add \
        --description $'Create a greeting file\n\nAdd hello.txt containing exactly: Hello from Orka.' \
        --assignee machine
)"
echo "node: $node_id"

echo "==> Running the node with Orka"
run_output="$(
    cd "$target_dir"
    "$orka_bin" run "$node_id"
)"
printf '%s\n' "$run_output"
candidate_id="$(
    printf '%s\n' "$run_output" |
        sed -n 's/^candidate \(candidate-[^ ]*\).*/\1/p' |
        head -n 1
)"
if [[ -z "$candidate_id" ]]; then
    echo "error: Orka did not report a candidate" >&2
    exit 1
fi

echo "==> Starting the candidate review"
review_output="$(
    cd "$target_dir"
    "$orka_bin" review start "$candidate_id"
)"
printf '%s\n' "$review_output"
verification_id="$(
    printf '%s\n' "$review_output" |
        sed -n 's/^verification \(node-[^ ]*\).*/\1/p' |
        head -n 1
)"
if [[ -z "$verification_id" ]]; then
    echo "error: Orka did not report a verification node" >&2
    exit 1
fi

echo "==> Accepting the review"
(
    cd "$target_dir"
    "$orka_bin" review finish "$verification_id" \
        --outcome accepted \
        --summary "Verified hello.txt contains the requested greeting."
)

candidate_view="$(
    cd "$target_dir"
    "$orka_bin" candidate "$candidate_id"
)"
artifact="$(
    printf '%s\n' "$candidate_view" |
        sed -n 's/^head[[:space:]]*//p' |
        head -n 1
)"
if [[ -z "$artifact" ]]; then
    echo "error: could not determine the candidate artifact" >&2
    exit 1
fi

echo "==> Publishing the accepted candidate"
(
    cd "$target_dir"
    "$orka_bin" publish "$candidate_id"
    "$orka_bin" audit
)

echo "==> Verifying the published change on main"
branch="$(git -C "$target_dir/project" branch --show-current)"
main_head="$(git -C "$target_dir/project" rev-parse main)"
file_contents="$(git -C "$target_dir/project" show main:hello.txt)"
project_status="$(git -C "$target_dir/project" status --porcelain)"
published_candidates="$(
    cd "$target_dir"
    "$orka_bin" candidates
)"

[[ "$branch" == main ]]
[[ "$main_head" == "$artifact" ]]
[[ "$file_contents" == "Hello from Orka." ]]
[[ -z "$project_status" ]]
printf '%s\n' "$published_candidates" |
    grep -Eq "^${candidate_id}[[:space:]].*[[:space:]]published[[:space:]].*->[[:space:]]main"
git -C "$target_dir/project" merge-base --is-ancestor "$artifact" main

echo
echo "PASS"
echo "repository:   $target_dir"
echo "work node:    $node_id"
echo "verification: $verification_id"
echo "candidate:    $candidate_id"
echo "main HEAD:    $main_head"
echo "hello.txt:    $file_contents"
