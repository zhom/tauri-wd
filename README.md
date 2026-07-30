<div align="center">
  <img src="https://zhom.github.io/tauri-wg/favicon.svg" alt="Tauri WebDriver" width="72">
  <h1>Tauri WebDriver</h1>
  <strong>Reliable end-to-end testing for Tauri on every desktop platform.</strong>
  <br>
  <a href="https://zhom.github.io/tauri-wg/">Website</a>
</div>
<br>

<p align="center">
  <a href="https://crates.io/crates/tauri-cross-platform-webdriver"><img alt="Crates.io" src="https://img.shields.io/crates/v/tauri-cross-platform-webdriver"></a>
  <a href="https://github.com/zhom/tauri-wg/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/zhom/tauri-wg/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
</p>

<img alt="Tauri WebDriver supports macOS, Windows, and Linux" src="https://zhom.github.io/tauri-wg/social-card.png">

## Features

- Native WKWebView, WebView2, and WebKitGTK automation
- Current W3C WebDriver protocol for WebdriverIO, Selenium, and Fantoccini
- Elements, actions, scripts, frames, shadow DOM, dialogs, cookies, screenshots, and PDF
- Isolated sessions, serialized commands, crash recovery, bounded payloads, and full process-tree cleanup
- Loopback-only endpoints with private per-session authentication

## Install

Add the plugin to a dedicated test feature:

```toml
[features]
e2e = ["dep:tauri-plugin-cross-platform-webdriver"]

[dependencies]
tauri-plugin-cross-platform-webdriver = { version = "0.1", optional = true }
```

```rust
let builder = tauri::Builder::default();

#[cfg(feature = "e2e")]
let builder = builder.plugin(tauri_plugin_cross_platform_webdriver::init());
```

Install the driver and point your W3C client at `127.0.0.1:4444`:

```sh
cargo install tauri-cross-platform-webdriver --locked
tauri-wd
```

```js
capabilities: [
  {
    "tauri:options": {
      application: "./target/debug/my-app",
    },
  },
];
```

Build the app with `--features e2e`. Never enable the plugin in a production binary.

## Platforms

macOS · Windows · Linux

## License

MIT
