# Repository Guidelines

## Project Structure & Module Organization

This Cargo workspace centers on `hftbacktest/`, the core Rust library; examples live in `hftbacktest/examples/`. `hftbacktest-derive/` provides procedural macros. `collector/`, `connector/`, and `tui/` are executable crates for data collection, exchange connectivity, and terminal monitoring. `py-hftbacktest/` contains PyO3 bindings, the Python package, and `tests/`. Documentation is in `docs/`; do not commit generated `target/` output.

## Build, Test, and Development Commands

- `cargo build --workspace` compiles every Rust crate in debug mode.
- `cargo build --release -p connector` builds the optimized connector; substitute `hftbacktest-tui` for the TUI.
- `cargo test --workspace` runs unit and integration tests across the workspace.
- `cargo fmt --all -- --check` verifies Rust formatting; run `cargo fmt --all` to apply it.
- `cargo clippy --workspace --all-targets` checks common Rust correctness and style issues.
- `cd py-hftbacktest && maturin develop` installs the native extension in the active environment.
- `cd py-hftbacktest && pytest` runs the Python binding tests.
- `cd docs && make html` builds the Sphinx documentation after installing `docs/requirements.txt`.

## Coding Style & Naming Conventions

Use four-space indentation, `snake_case` for modules/functions, `CamelCase` for types and traits, and `SCREAMING_SNAKE_CASE` for constants. Follow `rustfmt.toml` import grouping and Unix newlines. Python requires 3.11+; follow PEP 8 and add type hints to new public interfaces. Keep exchange-specific code in its provider module.

## Testing Guidelines

Place Rust unit tests beside code in `#[cfg(test)]` modules and integration tests in crate-level `tests/`. Name tests after the behavior they prove. Python tests follow pytest conventions (`test_*.py`, `test_*`). Add regression tests for fixes; run the narrow package test first, such as `cargo test -p connector`, then the workspace suite. No coverage threshold is configured.

## Commit & Pull Request Guidelines

History generally uses concise imperative Conventional Commit prefixes such as `feat:`, `fix:`, `test:`, `refactor:`, `ci:`, and `chore:`; optional scopes are accepted, for example `fix(tardis): ...`. Keep each commit focused. Pull requests should explain the problem and solution, identify affected crates or exchanges, link relevant issues, and list verification commands. Include screenshots for TUI changes and call out configuration, API, or data-format compatibility impacts.

## Security & Configuration

Never commit exchange credentials, private keys, or populated production configuration. Copy the `.toml.example` files under `connector/examples/`, keep secrets local, and use testnet or demo endpoints when validating connector changes.
