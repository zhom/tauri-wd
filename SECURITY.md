# Security

## Supported versions

Only the latest released minor version receives security fixes.

## Reporting a vulnerability

Use GitHub’s private vulnerability reporting for this repository. Do not open a public issue for an unpatched vulnerability.

Include the affected version and platform, a minimal reproduction, impact, and any known mitigations. You can expect an initial response within seven days.

## Automation boundary

`tauri-wd` can launch the executable and environment supplied in `tauri:options`. It therefore binds only to loopback. The embedded plugin is inert unless the app is launched for automation and protects its ephemeral endpoint with a per-session token.

The plugin belongs only in dedicated test builds. Never enable its Cargo feature in a production binary.

## Dependency advisories

CI fails on known vulnerabilities. RustSec also reports maintenance warnings inherited from Tauri’s current Linux GTK3 stack; these are reviewed separately and are not silently suppressed.
