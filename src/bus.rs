//! A small session-bus API so a desktop shell can be Felt's face without being Felt: `org.clockmost.Felt` at
//! `/org/clockmost/Felt` - `ShowChat()`, `Say(text)` (a line to her, as if typed in the chat window), a `LastLine`
//! property (JSON: what she last said), a `Status` property ("checking" / "online" / "offline"), an `Agenda`
//! property (JSON) for the shell's calendar, `GoToLedge()` (her body: onto the ledge nearest the cursor), and `Windows(json)` / `Cursor(json)` for our own KWin scripts
//! (screen reactions, her gaze; KWin-only callers). The bar's Felt chip uses it; with no such shell, the tray icon does the same job.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tao::event_loop::EventLoopProxy;
use tokio::sync::watch;
use zbus::message::Header;
use zbus::{connection, interface};

use crate::UiEvent;
use crate::screen::kwin::Snapshot;

/// 0 = checking, 1 = online, 2 = offline.
#[derive(Default)]
pub struct SharedStatus(AtomicU8);

impl SharedStatus {
    pub fn set(&self, online: bool) {
        self.0.store(if online { 1 } else { 2 }, Ordering::Relaxed);
    }

    pub fn text(&self) -> &'static str {
        match self.0.load(Ordering::Relaxed) {
            1 => "online",
            2 => "offline",
            _ => "checking",
        }
    }
}

/// The agenda JSON (an array of events) the bar's calendar shows; "[]" until the first fetch.
pub type SharedAgenda = Arc<Mutex<String>>;

struct FeltBus {
    proxy: Mutex<EventLoopProxy<UiEvent>>,
    chat: Arc<crate::chat::Chat>,
    last_line: crate::voice::LastLine,
    status: Arc<SharedStatus>,
    agenda: SharedAgenda,
    /// Window snapshots from our KWin script (screen reactions); None = off.
    windows: Option<watch::Sender<Snapshot>>,
    /// The cursor's desktop position from our KWin script (her gaze); None = no body.
    cursor: Option<watch::Sender<Option<(f64, f64)>>>,
    body: Option<crate::body::Handle>,
}

/// Only KWin itself (our scripts) may call the methods that feed screen capture and her gaze.
async fn from_kwin(hdr: &Header<'_>, conn: &zbus::Connection) -> zbus::fdo::Result<()> {
    let kwin = zbus::fdo::DBusProxy::new(conn)
        .await?
        .get_name_owner("org.kde.KWin".try_into().expect("valid name"))
        .await?;
    if hdr.sender().map(|s| s.as_str()) != Some(kwin.as_str()) {
        return Err(zbus::fdo::Error::AccessDenied(
            "only KWin may call this".into(),
        ));
    }
    Ok(())
}

#[interface(name = "org.clockmost.Felt")]
impl FeltBus {
    fn show_chat(&self) {
        let _ = self
            .proxy
            .lock()
            .expect("proxy lock")
            .send_event(UiEvent::ShowChat);
    }

    /// A line to her from the shell's talk box: sent like one typed in the chat window (she answers out loud).
    fn say(&self, text: String) -> zbus::fdo::Result<()> {
        let text = text.trim().to_owned();
        if text.is_empty() || text.chars().count() > 2000 {
            return Err(zbus::fdo::Error::InvalidArgs(
                "empty or over 2000 characters".into(),
            ));
        }
        let chat = self.chat.clone();
        tokio::spawn(async move {
            if let Err(e) = chat.send(&text).await {
                tracing::warn!("bus Say: {e:#}");
            }
        });
        Ok(())
    }

    /// Her body: clean house - every prop on the floor or in her hands dissolves.
    fn clear_props(&self) {
        if let Some(b) = &self.body {
            b.clear_props();
        }
    }

    /// Her body: go to the window ledge nearest the cursor and sit on it (bind it to a shortcut).
    fn go_to_ledge(&self) {
        if let Some(b) = &self.body {
            b.go_to_ledge();
        }
    }

    /// Called by our KWin script only: anyone else is refused (they could steer what gets captured).
    async fn windows(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        json: String,
    ) -> zbus::fdo::Result<()> {
        let Some(tx) = &self.windows else {
            return Ok(());
        };
        from_kwin(&hdr, conn).await?;
        let snap: Snapshot = serde_json::from_str(&json)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        tx.send_replace(snap);
        Ok(())
    }

    /// Called by our cursor script only (her gaze follows it): `json` is `[x, y]` in desktop pixels.
    async fn cursor(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        json: String,
    ) -> zbus::fdo::Result<()> {
        let Some(tx) = &self.cursor else {
            return Ok(());
        };
        from_kwin(&hdr, conn).await?;
        let [x, y]: [f64; 2] = serde_json::from_str(&json)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        tx.send_replace(Some((x, y)));
        Ok(())
    }

    /// Upcoming events (JSON array: title, calendar, start, end, all_day, location, link), refreshed every 5 min.
    #[zbus(property)]
    fn agenda(&self) -> String {
        self.agenda.lock().expect("agenda lock").clone()
    }

    /// What she last said: `{"text", "at" (unix seconds), "source" ("chat" / "ambient" / "screen")}`, "{}" before.
    #[zbus(property)]
    fn last_line(&self) -> String {
        self.last_line.lock().expect("last line lock").clone()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        self.status.text().into()
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn serve(
    proxy: EventLoopProxy<UiEvent>,
    chat: Arc<crate::chat::Chat>,
    last_line: crate::voice::LastLine,
    status: Arc<SharedStatus>,
    agenda: SharedAgenda,
    windows: Option<watch::Sender<Snapshot>>,
    cursor: Option<watch::Sender<Option<(f64, f64)>>>,
    body: Option<crate::body::Handle>,
) -> zbus::Result<zbus::Connection> {
    let bus = FeltBus {
        proxy: Mutex::new(proxy),
        chat,
        last_line,
        status,
        agenda,
        windows,
        cursor,
        body,
    };
    connection::Builder::session()?
        .name("org.clockmost.Felt")?
        .serve_at("/org/clockmost/Felt", bus)?
        .build()
        .await
}
