set shell := ["bash", "-cu"]
set windows-shell := ["cmd.exe", "/c"]

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

python := if os_family() == "windows" { "python" } else { "python3" }

# Rebuild the fixture repos and rewrite their manifests.
fixtures:
    {{python}} eval/generator/generate.py

# Rebuild the fixture repos and fail if any manifest would change.
fixtures-check:
    {{python}} eval/generator/generate.py --check

# Run the mutation-corpus validation harness.
eval *args: fixtures-check build
    {{python}} eval/runner/run.py {{args}} -- {{eval_engine}}

# Test the grading tool itself.
eval-selftest: fixtures-check
    {{python}} -m unittest discover --start-directory eval/runner/tests --top-level-directory eval/runner

eval_engine := if os_family() == "windows" { "engine/target/debug/marrow.exe" } else { "engine/target/debug/marrow" }
