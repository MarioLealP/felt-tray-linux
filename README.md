# felt-tray

Felt on a Linux desktop: the chat window (your Felt server's hosted app in a web view, created on open and destroyed
on close), her voice for typed replies, her alert lines as desktop notifications, and a tray icon - or, for a
desktop shell with its own Felt button, a D-Bus API instead. Works under any desktop with a StatusNotifierItem host
(Plasma, waybar, GNOME + AppIndicator, ...). Part of clockmost: it talks to a clockmost Felt server (`felt-core`).

## Set up

1. `pacman -S felt-tray` (the `[clockmost]` repo).
2. `felt-tray --init` writes `~/.config/felt-tray/config.toml` (commented; also in `/usr/share/doc/felt-tray/`).
   Fill in your Felt server's machine host, app URL, SSO host and the client-certificate file. felt-tray refuses to
   start while a required value is empty, and says which.
3. `systemctl --user enable --now felt-tray` (starts with your graphical session).

On the clockmost desktop, `clockmost-desktop-config` does steps 2-3 (`clockmost-desktop-setup`).

## What it needs from the Felt server

- **Machine host** (`felt_core_url`): a TLS proxy that requires the tray's client certificate and hands its
  identity to felt-core (clockmost: Caddy's `felt-api` host, WP-62 M2b). No tokens anywhere.
- **Client certificate** (`client_bundle`): one PEM file, key + certificate + chain, replaced in place when renewed
  - felt-tray re-reads it when it changes. clockmost: the vault renewer (`clockmost-renewer@tray.timer`) writes it;
  your user must be in its `tray-certs` group.
- **The hosted Felt app** (`app_url`) behind your SSO; the chat window remembers the login in
  `$XDG_DATA_HOME/felt-tray/webview`.

## Pieces

- **Chat window**: `window.feltTray.text_chat(message)` is defined on the app's origin only (init script + IPC,
  origin-checked again on the Rust side); the window only browses `window_hosts`, other links open in the default
  browser. A click always opens it fresh in front (no Wayland activation token through ksni).
- **Her voice**: WAV clips through the configured `player` (default `pw-play`, PipeWire); long replies stream
  sentence by sentence (WP-54); a new line stops the old reply. Voice down on the server = text only.
- **Her alert lines** (`notices = true`): felt-core decides what pops, sounds and stays; actions Details (chat at
  `#systems/<id>`) and Handled. Position in `$XDG_DATA_HOME/felt-tray/notices.json`; after a restart at most 5
  missed lines pop.
- **Her own lines** (`proactive = true`): long-polls felt-core's proactive channel (WP-25: calendar heads-ups,
  alarms, things she noticed) and speaks them; when her voice can't be fetched the line pops up as a
  notification instead of being re-queued forever.
- **Screen reactions** (`screen_reactions = true`, KDE Plasma only): our KWin script reports the visible windows
  (app id, title) on every change; on a focus change (3 s settle) and every `screen_sample_s` she picks a target
  over that LIST with the blocklist (`capture_blocklist.toml`, example in `/usr/share/doc/felt-tray/`) - blocked or
  unidentified windows are never captured, a missing blocklist means no captures - then captures only that window
  (KWin ScreenShot2, granted by `org.clockmost.felt-tray.desktop`), drops near-duplicates (dHash), and sends it to
  felt-core; she speaks the reaction. Skipped after 10 min idle. The decision log line never contains a rejected
  window's title. The `Windows` D-Bus method only accepts calls from KWin itself.
- **D-Bus API** (`org.clockmost.Felt` at `/org/clockmost/Felt`): `ShowChat()` and a `Status` property
  ("checking" / "online" / "offline"). With `tray_icon = false` a shell's own button uses it instead of the icon
  (clockmost-shell's Felt chip does).
- **Not yet**: push-to-talk (GlobalShortcuts portal), the raw-ntfy fallback when felt-core is down, her body (the
  overlay), the music/proactive watchers. Plan: `runbooks/felt-desktop-rust-port.md` (workspace repo).

Modules: `config` · `core` (felt-core client) · `chat` · `audio` · `tray` · `bus` · `window` · `notices` · `notify` · `screen` (`policy`, `kwin`) · `proactive`.
Checks: `just check`. Release: `./release.sh X.Y.Z` (pkgs CI builds, scans, publishes).
