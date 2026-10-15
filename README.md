# felt-tray (Rust, Linux)

Felt on the Linux desktop: a tray icon (StatusNotifierItem - works under any panel/shell), the chat window
(the hosted Felt app in a web view, created on open and destroyed on close), and her voice for typed replies.
Replaces the Windows Python `felt-tray` (gone with Windows, 2026-10-14). Plan + decisions:
`~/workspace/runbooks/felt-desktop-rust-port.md`.

- **Identity**: felt-core's machine host (`felt-api`) with the tray's 2-hour Felt client cert - the vault renewer
  (`clockmost-renewer@tray.timer`, system) writes `bundle.pem`; we re-read it when it changes. The user running
  the tray must be in `tray-certs`. No tokens.
- **Chat window**: `window.feltTray.text_chat(message)` is defined on the app's origin only (wry init script +
  IPC, origin-checked again on the Rust side); the window only browses `window_hosts`, other links open in the
  default browser. SSO cookie kept in `$XDG_DATA_HOME/felt-tray/webview`.
- **Voice out**: WAV clips through `pw-play` (PipeWire), streamed replies (WP-54) queued; a new line stops the
  old reply.
- **D-Bus API** (`bus.rs`): `org.clockmost.Felt` at `/org/clockmost/Felt` - `ShowChat()` + `Status` ("checking" /
  "online" / "offline"). clockmost-shell's Felt chip uses it; `tray_icon = false` then hides the tray icon. Another
  shell: set `tray_icon = true` and nothing else changes.
- **Not yet**: push-to-talk (needs the GlobalShortcuts portal spike S3), notices/pop-ups, the overlay, desktop
  watchers.

Config: `$XDG_CONFIG_HOME/felt-tray/config.toml` (see `config.example.toml`), or `FELT_TRAY_CONFIG`.
Modules: `config` · `core` (felt-core client) · `chat` · `audio` · `tray` · `bus` · `window`. Checks: `just check`.
