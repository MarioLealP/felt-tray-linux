#!/usr/bin/env bash
# What the pkgs CI scan runs (pkgs/packaging rust-pkg.yml), in its order, plus the tests - run before every push.
# trivy fs runs in CI only. Stops at the first failure.
set -euo pipefail
cd "$(dirname "$0")"
step() { printf '\n== %s\n' "$*"; "$@"; }
step cargo fmt --all --check
step cargo clippy --locked --all-targets --all-features -- -D warnings
step cargo machete --with-metadata
step cargo deny check
step cargo vet --locked
step uvx --quiet semgrep scan --config .semgrep.yml --config p/rust --error --metrics=off --quiet .
step cargo test --locked -q
printf '\nall green\n'
