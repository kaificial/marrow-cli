set shell := ["bash", "-cu"]

# Build the engine workspace.
build:
    cargo build --manifest-path engine/Cargo.toml --workspace

# Run engine tests.
test:
    cargo test --manifest-path engine/Cargo.toml --workspace

# Lint: fmt check + clippy deny warnings.
lint:
    cargo fmt --manifest-path engine/Cargo.toml --all -- --check
    cargo clippy --manifest-path engine/Cargo.toml --workspace --all-targets -- -D warnings

# Format the engine workspace.
fmt:
    cargo fmt --manifest-path engine/Cargo.toml --all

# Run the mutation-corpus validation harness.
eval:
    @echo "eval harness not built yet (see docs/BUILD_PLAN.md Phase 1)"
