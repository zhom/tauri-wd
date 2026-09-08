# tauri-wd

Reliable end-to-end testing for Tauri on every desktop platform.

[Website](https://zhom.github.io/tauri-wd/)

## Features

- Native WKWebView, WebView2, and WebKitGTK automation
- Current W3C WebDriver protocol for WebdriverIO, Selenium, and Fantoccini
- Elements, actions, scripts, frames, shadow DOM, dialogs, cookies, screenshots, and PDF
- Isolated sessions, serialized commands, crash recovery, bounded payloads, and full process-tree cleanup
- Loopback-only endpoints with private per-session authentication
- Headless sessions that never show a window or steal focus

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

## Headless

Run a session without a visible window, so a suite never pops up a window or
steals focus while you work. Add `headless` to `tauri:options`:

```js
capabilities: [
  {
    "tauri:options": {
      application: "./target/debug/my-app",
      headless: true,
    },
  },
];
```

On macOS the plugin keeps the window on screen but fully transparent,
click-through and never focused, floating above other windows on every Space.
That is deliberate: a hidden or off-screen window makes WebKit suspend rendering
and `requestAnimationFrame`, which stalls any animation-gated test. The webview
still lays out, runs scripts, receives synthesized input, and renders for
screenshots, so every WebDriver command behaves as it does with a visible
window. On Windows and Linux the window is hidden instead, and the page's
`requestAnimationFrame` may pause while it is; tests that wait on animations
there should not rely on headless yet.

The plugin only runs once a webview is ready, so it cannot undo what creating
the window already did. A window built visible and focused activates the app
and takes key focus for that moment, and tao resets the activation policy at
launch, so a Dock tile can appear. For a session that changes nothing on your
screen, do both of these in the app itself when `tauri_wd::headless_enabled()`:

```rust
let headless = tauri_wd::headless_enabled();
#[cfg(target_os = "macos")]
if headless {
    // Through `App`, so it lands in tao's own launch state.
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}
tauri::WebviewWindowBuilder::new(app, "main", Default::default())
    .visible(!headless)
    .focused(!headless)
    .build()?;
```

## Platforms

macOS · Windows · Linux

## License

MIT
