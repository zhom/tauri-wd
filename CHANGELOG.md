# Changelog

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
