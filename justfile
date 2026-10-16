# `just` lists recipes; `just check` = what CI runs

default:
    @just --list

# exactly what the pkgs CI scan runs, plus the tests
check:
    ./ci-check.sh

fmt:
    cargo fmt

fmt-check:
    cargo fmt --check

lint:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo nextest run --no-tests=warn

deny:
    cargo deny check

cov:
    cargo llvm-cov nextest --html
