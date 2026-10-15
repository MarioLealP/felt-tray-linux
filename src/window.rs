//! The chat window: the hosted Felt app in a web view, created when opened and destroyed when closed (no
//! idle cost). Its SSO cookie lives in our data dir, so the login is remembered. A line typed in the app
//! reaches us through `window.feltTray.text_chat()` (only on the app's own origin) so she speaks the reply.
//! The window only browses the configured hosts; any other link opens in the default browser.

use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Url;
use serde::Deserialize;
use tao::dpi::LogicalSize;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::platform::unix::WindowExtUnix;
use tao::window::{Window, WindowBuilder};
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder, WebViewBuilderExtUnix};

use crate::UiEvent;
use crate::chat::Chat;

pub struct ChatWindow {
    // owned so the window lives as long as the web view in it
    _window: Window,
    webview: WebView,
}

/// What stays the same across window openings.
pub struct WindowSetup {
    pub app_url: Url,
    pub hosts: Arc<Vec<String>>,
    pub chat: Arc<Chat>,
    pub rt: tokio::runtime::Handle,
    pub proxy: EventLoopProxy<UiEvent>,
}

#[derive(Deserialize)]
struct IpcCall {
    id: u64,
    op: String,
    message: String,
}

impl ChatWindow {
    pub fn open(
        target: &EventLoopWindowTarget<UiEvent>,
        ctx: &mut WebContext,
        s: &WindowSetup,
        fragment: Option<&str>,
    ) -> Result<Self> {
        let window = WindowBuilder::new()
            .with_title("Felt")
            .with_inner_size(LogicalSize::new(800.0, 600.0))
            .build(target)?;
        let origin = s.app_url.origin().ascii_serialization();
        let (chat, rt, proxy, ipc_origin) = (
            s.chat.clone(),
            s.rt.clone(),
            s.proxy.clone(),
            origin.clone(),
        );
        let (nav_hosts, popup_hosts) = (s.hosts.clone(), s.hosts.clone());
        let webview = WebViewBuilder::new_with_web_context(ctx)
            .with_url(match fragment {
                Some(f) => format!("{}{f}", s.app_url.as_str().split('#').next().unwrap_or("")),
                None => s.app_url.to_string(),
            })
            .with_initialization_script(bridge_script(&origin))
            .with_ipc_handler(move |req| {
                // Linux reports the main frame's URL here; only the app itself may talk to the tray.
                let from =
                    Url::parse(&req.uri().to_string()).map(|u| u.origin().ascii_serialization());
                if from.as_deref() != Ok(ipc_origin.as_str()) {
                    return tracing::warn!("ipc from {:?} refused", req.uri());
                }
                let Ok(call) = serde_json::from_str::<IpcCall>(req.body()) else {
                    return tracing::warn!("bad ipc message");
                };
                if call.op != "text_chat" {
                    return tracing::warn!("unknown ipc op {}", call.op);
                }
                let (chat, proxy) = (chat.clone(), proxy.clone());
                rt.spawn(async move {
                    let error = match chat.send(&call.message).await {
                        Ok(()) => None,
                        Err(e) => {
                            tracing::warn!("text chat failed: {e:#}");
                            Some("couldn't reach Felt".to_owned())
                        }
                    };
                    let _ = proxy.send_event(UiEvent::Resolve { id: call.id, error });
                });
            })
            .with_navigation_handler(move |url| allowed(&url, &nav_hosts) || open_outside(&url))
            .with_new_window_req_handler(move |url, _| {
                if allowed(&url, &popup_hosts) {
                    NewWindowResponse::Allow
                } else {
                    open_outside(&url);
                    NewWindowResponse::Deny
                }
            })
            .build_gtk(window.default_vbox().context("no GTK box in the window")?)?;
        Ok(Self {
            _window: window,
            webview,
        })
    }

    pub fn resolve(&self, id: u64, error: Option<&str>) {
        let result = match error {
            None => serde_json::json!({ "ok": true }),
            Some(e) => serde_json::json!({ "ok": false, "error": e }),
        };
        let js = format!("window.__feltTrayResolve && window.__feltTrayResolve({id}, {result})");
        if let Err(e) = self.webview.evaluate_script(&js) {
            tracing::warn!("can't hand the result back to the app: {e}");
        }
    }
}

fn allowed(url: &str, hosts: &[String]) -> bool {
    Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https" && u.host_str().is_some_and(|h| hosts.iter().any(|a| a == h))
    }) || url == "about:blank"
}

/// Always false: the window itself doesn't go there.
fn open_outside(url: &str) -> bool {
    if url.starts_with("https://") || url.starts_with("http://") {
        // nosemgrep: clockmost-shell-out  fixed binary, one argv element (an http(s) URL), no shell
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    false
}

/// `window.feltTray.text_chat(message) -> Promise<{ok, error?}>`, defined only on the app's own origin.
fn bridge_script(origin: &str) -> String {
    let origin = serde_json::to_string(origin).expect("string");
    format!(
        r#"(() => {{
  if (window.location.origin !== {origin}) return;
  const pending = new Map();
  let next = 0;
  window.__feltTrayResolve = (id, result) => {{
    const done = pending.get(id);
    if (done) {{ pending.delete(id); done(result); }}
  }};
  window.feltTray = Object.freeze({{
    text_chat(message) {{
      return new Promise((resolve) => {{
        const id = ++next;
        pending.set(id, resolve);
        window.ipc.postMessage(JSON.stringify({{ id, op: "text_chat", message: String(message) }}));
      }});
    }},
  }});
}})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_on_listed_hosts() {
        let hosts = vec!["app.test".to_owned(), "sso.test".to_owned()];
        assert!(allowed("https://app.test/app/#talk", &hosts));
        assert!(allowed("https://sso.test/if/flow/login/", &hosts));
        assert!(!allowed("http://app.test/", &hosts));
        assert!(!allowed("https://app.test.evil/", &hosts));
        assert!(!allowed("https://evil.test/?https://app.test", &hosts));
    }

    #[test]
    fn bridge_is_origin_guarded() {
        let js = bridge_script("https://app.test");
        assert!(js.contains(r#"window.location.origin !== "https://app.test""#));
    }
}
