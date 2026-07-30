# Contributing

Install the current stable Rust toolchain and the native Tauri prerequisites for your operating system.

Before opening a pull request, run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
```

Run the native conformance test with `bash tests/e2e-smoke.sh` on macOS or Linux, or `./tests/e2e-smoke.ps1` on Windows. Linux also needs WebKitGTK 4.1 and a display such as Xvfb.

Protocol fixes should include a regression test. Platform-specific changes should explain which native webview API is involved and must not weaken the loopback, token, or test-build boundaries.
