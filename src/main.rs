//! felt-tray: Felt on the Linux desktop - a tray icon, the chat window (the hosted Felt app), and her voice
//! for typed replies. Voice input, the overlay and the desktop watchers come later
//! (`runbooks/felt-desktop-rust-port.md`).

mod audio;
mod chat;
mod config;
mod core;
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
    let cfg = config::load()?;
    let rt = tokio::runtime::Runtime::new()?;
    let _guard = rt.enter();

    let core = Arc::new(core::FeltCore::new(
        cfg.felt_core_url.clone(),
        cfg.client_bundle.clone(),
    ));
    let chat = chat::Chat::new(core.clone(), audio::Player::spawn(), cfg.session_id.clone());

    let event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let tray = rt
        .block_on(tray::FeltTray::new(proxy.clone()).spawn())
        .context("can't put the tray icon up (no StatusNotifier host?)")?;
    let status_tray = tray.clone();
    rt.spawn(async move {
        loop {
            let online = core.ping().await;
            if status_tray
                .update(|t| t.online = Some(online))
                .await
                .is_none()
            {
                break; // tray gone
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    let setup = WindowSetup {
        app_url: cfg.app_url.parse().context("app_url is not a URL")?,
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
            Event::UserEvent(UiEvent::ShowChat) => match &win {
                Some(w) => w.window.set_focus(),
                None => match ChatWindow::open(target, &mut ctx, &setup) {
                    Ok(w) => win = Some(w),
                    Err(e) => tracing::error!("can't open the chat window: {e:#}"),
                },
            },
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
                let t = tray.clone();
                rt.block_on(async move { t.shutdown().await });
                *flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
