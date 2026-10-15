//! Desktop notifications (org.freedesktop.Notifications) - whatever notification server the desktop runs
//! draws them (on ours: the bar's own pop-ups). Action clicks come back as [`Action`]s.

use std::collections::HashMap;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::zvariant::Value;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;
}

pub struct Popup<'a> {
    pub summary: &'a str,
    pub body: &'a str,
    /// (key, label) pairs; "default" = a click on the pop-up itself.
    pub actions: &'a [(&'a str, &'a str)],
    pub critical: bool,
    /// Stays until dismissed.
    pub sticky: bool,
    pub sound: bool,
}

/// A clicked action: the notification id it belongs to, and its key.
pub type Action = (u32, String);

#[derive(Clone)]
pub struct Notifier {
    proxy: NotificationsProxy<'static>,
}

impl Notifier {
    /// Connects to the session bus; clicked actions arrive on the returned channel.
    pub async fn connect() -> zbus::Result<(Self, mpsc::UnboundedReceiver<Action>)> {
        let conn = zbus::Connection::session().await?;
        let proxy = NotificationsProxy::new(&conn).await?;
        let mut stream = proxy.receive_action_invoked().await?;
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(sig) = stream.next().await {
                if let Ok(a) = sig.args()
                    && tx.send((a.id, a.action_key)).is_err()
                {
                    break;
                }
            }
        });
        Ok((Self { proxy }, rx))
    }

    pub async fn show(&self, p: &Popup<'_>) -> zbus::Result<u32> {
        let actions: Vec<&str> = p.actions.iter().flat_map(|(k, l)| [*k, *l]).collect();
        let mut hints = HashMap::new();
        hints.insert("urgency", Value::U8(if p.critical { 2 } else { 1 }));
        hints.insert("desktop-entry", Value::from("felt-tray"));
        if p.sound {
            hints.insert("sound-name", Value::from("dialog-warning"));
        }
        let timeout = if p.sticky { 0 } else { -1 };
        self.proxy
            .notify("Felt", 0, "", p.summary, p.body, &actions, hints, timeout)
            .await
    }
}
