# tauri-wd

Reliable end-to-end testing for Tauri on every desktop platform.

[Website](https://zhom.github.io/tauri-wd/)

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
e2e = ["dep:tauri-wd"]

[dependencies]
tauri-wd = { version = "0.1", optional = true }
```

```rust
let builder = tauri::Builder::default();

#[cfg(feature = "e2e")]
let builder = builder.plugin(tauri_wd::init());
```

Install the driver and point your W3C client at `127.0.0.1:4444`:

```sh
cargo install tauri-wd --locked
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
