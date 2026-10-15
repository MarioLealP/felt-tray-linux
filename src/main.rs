//! felt-tray: Felt on the Linux desktop - a tray icon (optional; a shell's Felt button can use the D-Bus API
//! instead), the chat window (the hosted Felt app), and her voice for typed replies. Voice input, the overlay and the desktop watchers come later
//! (`runbooks/felt-desktop-rust-port.md`).

mod audio;
mod bus;
mod chat;
mod config;
mod core;
mod notices;
mod notify;
mod proactive;
mod screen;
mod tray;
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
    let chat = chat::Chat::new(
        core.clone(),
        audio::Player::spawn(cfg.player.clone()),
        cfg.session_id.clone(),
    );

    let event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let status = Arc::new(bus::SharedStatus::default());
    let (windows_tx, windows_rx) = if cfg.screen_reactions {
        let (tx, rx) = tokio::sync::watch::channel(screen::kwin::Snapshot::default());
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let bus_conn = rt
        .block_on(bus::serve(proxy.clone(), status.clone(), windows_tx))
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

    let notifier = rt.block_on(notify::Notifier::connect());
    if cfg.proactive {
        let n = notifier.as_ref().ok().map(|(n, _)| n.clone());
        rt.spawn(proactive::run(core.clone(), chat.player(), n));
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

    if let Some(rx) = windows_rx {
        let screen = screen::Screen {
            core: core.clone(),
            player: chat.player(),
            conn: bus_conn.clone(),
            blocklist: cfg.blocklist_path()?,
            sample: Duration::from_secs(cfg.screen_sample_s.max(10)),
            session_id: cfg.session_id.clone(),
        };
        rt.spawn(screen.run(rx));
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
                rt.block_on(async move { screen::kwin::unload_script(&c).await.ok() });
                if let Some(t) = tray.clone() {
                    rt.block_on(async move { t.shutdown().await });
                }
                *flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
