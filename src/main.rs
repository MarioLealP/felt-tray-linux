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
    let _bus = rt
        .block_on(bus::serve(proxy.clone(), status.clone()))
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

    match rt.block_on(notify::Notifier::connect()) {
        Ok(_) if !cfg.notices => {}
        Ok(n) => notices::start(
            core.clone(),
            n,
            proxy.clone(),
            config::data_dir()?.join("notices.json"),
        ),
        Err(e) => tracing::warn!("no desktop notifications, her alerts won't pop up: {e}"),
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
                if let Some(t) = tray.clone() {
                    rt.block_on(async move { t.shutdown().await });
                }
                *flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
