//! felt-tray: Felt on the Linux desktop - a tray icon (optional; a shell's Felt button can use the D-Bus API
//! instead), the chat window (the hosted Felt app), her voice, and her body (the Godot overlay). Voice input comes later
//! (`runbooks/felt-desktop-rust-port.md`).

mod audio;
mod body;
mod bus;
mod chat;
mod config;
mod controls;
mod core;
mod hotkeys;
mod mic;
mod notices;
mod notify;
mod proactive;
mod screen;
mod tray;
mod voice;
mod window;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ksni::TrayMethods;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use wry::WebContext;

use crate::window::{ChatWindow, WindowSetup};

#[derive(Debug)]
pub enum UiEvent {
    ShowChat,
    /// Open the chat window at a place in the app, e.g. `#systems/12` (a pop-up's details).
    OpenAt(String),
    Quit,
    /// A `text_chat` call from the app finished; hand the result back to its promise.
    Resolve {
        id: u64,
        error: Option<String>,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "felt_tray=info".into()),
        )
        .init();
    // nosemgrep: rust.lang.security.args.args  a CLI flag (--init), not a security decision
    if std::env::args().nth(1).as_deref() == Some("--init") {
        let p = config::init()?;
        println!(
            "wrote {} - fill in your Felt server's details, then start felt-tray",
            p.display()
        );
        return Ok(());
    }
    let rt = tokio::runtime::Runtime::new()?;
    let _guard = rt.enter();
    let cfg = match config::load()? {
        config::Loaded::Ok(c) => *c,
        config::Loaded::Missing(p) => {
            let msg = format!(
                "No config yet. Run `felt-tray --init`, then fill in {}.",
                p.display()
            );
            if let Ok((n, _)) = rt.block_on(notify::Notifier::connect()) {
                let popup = notify::Popup {
                    summary: "Felt isn't set up",
                    body: &msg,
                    actions: &[],
                    critical: false,
                    sticky: true,
                    sound: false,
                };
                let _ = rt.block_on(n.show(&popup));
            }
            anyhow::bail!(msg);
        }
    };

    let core = Arc::new(core::FeltCore::new(
        cfg.felt_core_url.clone(),
        cfg.client_bundle.clone(),
    ));
    let player = audio::Player::spawn(cfg.player.clone());
    // KWin's window list: screen reactions and her body (games, later her world) both read it.
    let (windows_tx, windows_rx) = if cfg.screen_reactions || cfg.body.is_some() {
        let (tx, rx) = tokio::sync::watch::channel(screen::kwin::Snapshot::default());
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    // Her body (the Godot overlay): its own thread; the overlay quits when felt-tray does. It follows the
    // cursor our KWin script reports over D-Bus.
    let (cursor_tx, body) = match (&cfg.body, &windows_rx) {
        (Some(b), Some(w)) => {
            let (tx, rx) = tokio::sync::watch::channel(None);
            let handle = body::start(b, core.clone(), player.clone(), rx, w.clone())?;
            (Some(tx), Some(handle))
        }
        _ => (None, None),
    };
    let with_body = body.is_some();
    let body_bus = body.clone();
    let body_keys = body.clone();
    let voice = voice::Voice::new(player, body);
    let chat = chat::Chat::new(core.clone(), voice.clone(), cfg.session_id.clone());
    let chat_keys = chat.clone();
    // "look at this" (the shortcut) -> the screen loop
    let (look_tx, look_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    let event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let status = Arc::new(bus::SharedStatus::default());
    let agenda: bus::SharedAgenda = Arc::new(std::sync::Mutex::new("[]".into()));
    let bus_conn = rt
        .block_on(bus::serve(
            proxy.clone(),
            status.clone(),
            agenda.clone(),
            windows_tx,
            cursor_tx,
            body_bus,
        ))
        .context("can't claim org.clockmost.Felt on the session bus (already running?)")?;
    let tray = if cfg.tray_icon {
        Some(
            rt.block_on(tray::FeltTray::new(proxy.clone()).spawn())
                .context("can't put the tray icon up (no StatusNotifier host?)")?,
        )
    } else {
        None
    };
    let (status_tray, ping_core) = (tray.clone(), core.clone());
    rt.spawn(async move {
        loop {
            let online = ping_core.ping().await;
            status.set(online);
            if let Some(t) = &status_tray {
                t.update(|t| t.online = Some(online)).await;
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    if cfg.agenda_days > 0 {
        let (core, days) = (core.clone(), cfg.agenda_days);
        rt.spawn(async move {
            loop {
                match core.calendar_upcoming(days).await {
                    Ok(v) => *agenda.lock().expect("agenda lock") = v.to_string(),
                    Err(e) => tracing::debug!("agenda: {e:#}"),
                }
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        });
    }
    let switches = Arc::new(controls::Switches::default());
    rt.spawn(controls::run(core.clone(), switches.clone(), voice.clone()));
    let notifier = rt.block_on(notify::Notifier::connect());
    if cfg.proactive {
        let n = notifier.as_ref().ok().map(|(n, _)| n.clone());
        rt.spawn(proactive::run(
            core.clone(),
            voice.clone(),
            n,
            switches.clone(),
        ));
    }
    match notifier {
        Ok(_) if !cfg.notices => {}
        Ok(n) => notices::start(
            core.clone(),
            n,
            proxy.clone(),
            config::data_dir()?.join("notices.json"),
        ),
        Err(e) => tracing::warn!("no desktop notifications, her alerts won't pop up: {e}"),
    }

    // KWin: keeps her overlay windows above; feeds screen reactions when they're on. Harmless elsewhere.
    if let Err(e) = rt.block_on(screen::kwin::load_script(&bus_conn)) {
        tracing::info!("no KWin script (not on KDE?): {e:#}");
    }
    if with_body && let Err(e) = rt.block_on(body::load_cursor_script(&bus_conn)) {
        tracing::info!(
            "no KWin cursor feed (not on KDE?) - her eyes won't follow the mouse: {e:#}"
        );
    }
    if cfg.screen_reactions
        && let Some(rx) = windows_rx
    {
        let screen = screen::Screen {
            core: core.clone(),
            voice: voice.clone(),
            conn: bus_conn.clone(),
            blocklist: cfg.blocklist_path()?,
            sample: Duration::from_secs(cfg.screen_sample_s.max(10)),
            session_id: cfg.session_id.clone(),
            switches: switches.clone(),
        };
        rt.spawn(screen.run(rx, look_rx));
    }

    if cfg.hotkeys {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let conn = bus_conn.clone();
        rt.spawn(async move {
            if let Err(e) = hotkeys::run(conn, tx).await {
                tracing::warn!("hotkeys: {e:#}");
            }
        });
        let (proxy, body) = (proxy.clone(), body_keys.clone());
        rt.spawn(async move {
            while let Some(a) = rx.recv().await {
                use hotkeys::Action;
                match a {
                    Action::Chat => {
                        let _ = proxy.send_event(UiEvent::ShowChat);
                    }
                    Action::Ledge => body.iter().for_each(body::Handle::go_to_ledge),
                    Action::Clean => body.iter().for_each(body::Handle::clear_props),
                    Action::Voice => {
                        let chat = chat_keys.clone();
                        tokio::spawn(async move {
                            if let Err(e) = chat.toggle_voice().await {
                                tracing::warn!("voice: {e:#}");
                            }
                        });
                    }
                    Action::Look => {
                        let _ = look_tx.send(());
                    }
                    a => {
                        let what = match a {
                            Action::Hide => "hide",
                            Action::Brb => "brb",
                            Action::Move => "move",
                            Action::Come => "come",
                            Action::Repeat => "repeat",
                            _ => "stop",
                        };
                        if let Some(b) = &body {
                            b.act(what, &serde_json::Value::Null);
                        }
                    }
                }
            }
        });
    }

    let setup = WindowSetup {
        app_url: cfg.app_url.parse().context("app_url is not a URL")?,
        size: cfg.window_size,
        hosts: Arc::new(cfg.window_hosts),
        chat,
        rt: rt.handle().clone(),
        proxy,
    };
    let mut ctx = WebContext::new(Some(config::data_dir()?.join("webview")));
    let mut win: Option<ChatWindow> = None;

    event_loop.run(move |event, target, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(e @ (UiEvent::ShowChat | UiEvent::OpenAt(_))) => {
                // ksni can't take the bar's xdg-activation token, so an existing window can't raise
                // itself on Wayland; a fresh window always maps in front (the app reloads its history).
                let fragment = match &e {
                    UiEvent::OpenAt(f) => Some(f.as_str()),
                    _ => None,
                };
                win = None;
                match ChatWindow::open(target, &mut ctx, &setup, fragment) {
                    Ok(w) => win = Some(w),
                    Err(e) => tracing::error!("can't open the chat window: {e:#}"),
                }
            }
            Event::UserEvent(UiEvent::Resolve { id, error }) => {
                if let Some(w) = &win {
                    w.resolve(id, error.as_deref());
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => win = None,
            Event::UserEvent(UiEvent::Quit) => {
                win = None;
                let c = bus_conn.clone();
                rt.block_on(async move {
                    screen::kwin::unload_script(&c).await.ok();
                    if with_body {
                        body::unload_cursor_script(&c).await.ok();
                    }
                });
                if let Some(t) = tray.clone() {
                    rt.block_on(async move { t.shutdown().await });
                }
                *flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
