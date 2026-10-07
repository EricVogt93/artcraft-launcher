# Contributing

Work on `development`, promote changes through `staging`, and open a pull request into `main` when ready. `main` requires an approving review, resolved conversations and the **All platforms** CI check; direct pushes, force pushes and deletion are blocked, including for administrators. `staging` and `development` cannot be deleted or force pushed.

## Build and test

Install Rust 1.95 or newer and the native dependencies listed in the [README](README.md). CI uses Rust 1.99.0.

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --bins
```

Keep blocking work on the core worker thread. Reuse native UI controls and validate Light/Dark appearance for UI changes. Tests should exercise failures and user-visible guarantees such as checksum validation, persisted settings, update pause/resume and preservation of existing files.

CI runs for pull requests targeting `main` and again after they merge. It builds every supported native target and uploads archives, installers, checksums and unsigned update manifests. The separate **Build and qualify native platforms** workflow is manually dispatched for graphics-session and installer qualification; successful compilation does not certify runtime behavior.

Do not commit `target/`, `.tmp/`, credentials, private signing keys or downloaded application packages. Use an ignored `.tmp/` folder or a temporary directory for test artifacts and clean them up. Contributions use Apache-2.0, as described in [LICENSE](LICENSE); third-party assets retain their own licenses.
