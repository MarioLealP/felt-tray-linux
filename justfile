# `just` lists recipes; `just check` = what CI runs

default:
    @just --list

# format + lint + test + supply chain, same as CI
check: fmt-check lint test deny shear

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

shear:
    cargo shear

cov:
    cargo llvm-cov nextest --html
