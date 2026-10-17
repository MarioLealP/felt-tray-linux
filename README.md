# felt-tray

Felt on a Linux desktop: the chat window (your Felt server's hosted app in a web view, created on open and destroyed
on close), her voice for typed replies, her alert lines as desktop notifications, her body (the Godot overlay that
draws her, optional), and a tray icon - or, for a
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
- **Hide / quiet switches** (WP-42.5, from the Felt app anywhere): polled every 5 s from felt-core. Quiet cuts
  her off mid-sentence and stops everything unprompted (screen reactions, her own lines - urgent ones still come
  through; items go back to felt-core); hide does the same until there's a body to hide. Heartbeat every 15 s to
  `/v1/controls/desktop` so the app shows the PC as on.
- **Screen reactions** (`screen_reactions = true`, KDE Plasma only): our KWin script reports the visible windows
  (app id, title) on every change; on a focus change (3 s settle) and every `screen_sample_s` she picks a target
  over that LIST with the blocklist (`capture_blocklist.toml`, example in `/usr/share/doc/felt-tray/`) - blocked or
  unidentified windows are never captured, a missing blocklist means no captures - then captures only that window
  (KWin ScreenShot2, granted by `org.clockmost.felt-tray.desktop`), drops near-duplicates (dHash), and sends it to
  felt-core; she speaks the reaction. Skipped after 10 min idle. The decision log line never contains a rejected
  window's title. The `Windows` D-Bus method only accepts calls from KWin itself.
- **Her body** (a `[body]` table: the overlay command + its animation library; needs `godot` and
  `rhubarb-lip-sync`): felt-tray launches the felt-godot-overlay project and drives it over a loopback socket (a
  fresh secret on its stdin each launch; it quits with felt-tray; relaunched with backoff if it dies). It runs on
  its own single-threaded thread (`src/body/`, the logic is the `felt-*` crates in `crates/`): she thinks while a
  line is out; each sentence gets Rhubarb mouth cues that go out the moment its audio really starts; the first
  brings a gesture (felt-core's tag, else felt-llm's pick for your own lines, else her talking gesture) and a face;
  her eyes follow the mouse (KDE: our `felt-tray-cursor` KWin script), look at you while she talks; her mood
  drifts, sets her resting face (`$XDG_DATA_HOME/felt-tray/mood.json`) and goes to felt-core's diary with her
  other events. On KDE a fullscreen game hides even keep-above windows on its monitor: give the overlay
  `-- --home-screen=right --only-home-screen` to keep her on one monitor.
- **D-Bus API** (`org.clockmost.Felt` at `/org/clockmost/Felt`): `ShowChat()`, a `Status` property
  ("checking" / "online" / "offline") and an `Agenda` property: the next `agenda_days` of her calendar as JSON
  (felt-core `/v1/calendar/upcoming`, refreshed every 5 min) for a shell's calendar - she reminds, it only shows. With `tray_icon = false` a shell's own button uses it instead of the icon
  (clockmost-shell's Felt chip does).
- **Her body, also**: idle variety and ambient actions (walks, sitting, sleepy late at night); the hide switch
  walks her off screen, quiet (= hide + hush) adds "zip it" and a line when it's lifted (`sounds`: her recorded
  clips); she notices games (KWin's `steam_app_*` windows + games.toml), AFK and GPU heat (amdgpu edge) through
  the reaction table and felt-core's `/v1/react/event`; she bobs to music (`music_capture`: PipeWire's monitor,
  in memory only).
- **Her world** (KDE): window tops are ledges (KWin's window list with positions, top-most first, and the work
  areas); she wanders up onto them now and then, sits on edges, drops when one goes; D-Bus `GoToLedge()` sends her
  to the ledge nearest the cursor. **Outfits** (data/outfits.json: dates, night pyjamas, a daily pick, rare
  swaps; `tees`: the day's approved tee; state in `$XDG_DATA_HOME/felt-tray/wardrobe.json`). **Face life**
  (micro-expressions, brow raises on her voice's peaks) and a filler "hmm" 1.5 s into a slow reply. **Calls**
  (`call_apps`: mic open = on a call): her voice ducks `duck_db` while someone talks.
- **Props** (data/props.json): her chest, things she fetches or conjures (a controller when a game starts,
  headphones with music, the phone, a water bottle, an evening lamp, a late-night blanket), clutter she tidies
  or kicks, a chair she sits on; placed items persist in `$XDG_DATA_HOME/felt-tray/placed_props.json`; D-Bus
  `ClearProps()` = clean house. Windows: she rides a dragged one and grumbles when hers goes.
- **Not yet**: push-to-talk (GlobalShortcuts portal), the raw-ntfy fallback when felt-core is down; for her body
  key-mash, mouth-along, accessories, weather props. Plan: `runbooks/felt-desktop-rust-port.md` (workspace repo).

Modules: `config` · `core` (felt-core client) · `chat` · `voice` · `audio` · `tray` · `bus` · `window` · `notices` · `notify` · `screen` (`policy`, `kwin`) · `proactive` · `controls` · `body` (`overlay`, `speech`, `reply`, `lipsync`, `gaze`, `mood`, `events`, `assets`, `clock`, `ambient`, `moves`, `presence`, `hush`, `react`, `signals`, `music`, `world`, `nav`, `ledge`, `outfits`, `tees`, `face`, `lines`, `call`, `window_events`, `props`).
Checks: `just check`. Release: `./release.sh X.Y.Z` (pkgs CI builds, scans, publishes).
