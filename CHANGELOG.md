# Changelog

## 0.2.2 (2026-10-14)

### Fixes

- build with tauri 2.12

### Maintenance

- version bump
- check latest deps


## 0.2.2

- Build against Tauri 2.12 and its WebView2 crates (`webview2-com` 0.39,
  `windows` 0.62). With an app on Tauri 2.12, 0.2.1 did not compile on Windows.
  Tauri 2.12.1 is now the minimum.
- No lint errors with Rust 1.99 (`async-trait` 0.1.92).

## 0.1.12

- Add headless sessions. Pass `tauri:options.headless: true` and the driver runs
  the app with no visible window. On macOS the plugin keeps the window on
  screen but transparent, click-through and never focused, so WebKit keeps
  rendering (a hidden window would pause `requestAnimationFrame`), and runs
  the app as an accessory; on Windows and Linux the window is hidden. The
  webview still lays out, runs scripts, receives synthesized input, and renders
  for screenshots. The plugin cannot undo a window created visible and focused,
  so an app that wants nothing to change on screen reads
  `tauri_wd::headless_enabled()`, creates its main window `.visible(false)` /
  `.focused(false)`, and on macOS sets `ActivationPolicy::Accessory` through
  `App::set_activation_policy` in its own setup (see the README).
