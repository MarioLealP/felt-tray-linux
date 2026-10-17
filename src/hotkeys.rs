//! Global shortcuts through the desktop portal (org.freedesktop.portal.GlobalShortcuts): the Wayland way for
//! an app to get keys while it isn't focused. felt-tray registers its app id with the host registry first (it
//! isn't sandboxed), then binds its actions with no keys of their own (Mario 2026-10-17: the Windows Ctrl+Alt
//! set clashed, and most aren't wanted) - give the ones you want a key in the desktop's shortcut settings
//! (KDE: System Settings -> Shortcuts -> felt-tray).

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

const APP_ID: &str = "org.clockmost.felt-tray";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Talk to her: press to start, press again to send.
    Voice,
    Chat,
    Hide,
    Brb,
    Move,
    Come,
    Look,
    Repeat,
    Stop,
    Clean,
    Ledge,
}

/// (id, what the desktop shows, the action) - the Windows tray's set, no keys assigned.
const SHORTCUTS: [(&str, &str, Action); 11] = [
    ("voice", "Talk to Felt (press again to send)", Action::Voice),
    ("text", "Open Felt's chat", Action::Chat),
    ("hide", "Hide / show Felt", Action::Hide),
    ("brb", "Felt: be right back", Action::Brb),
    ("move", "Felt: shoo (move over)", Action::Move),
    ("come", "Felt: come here", Action::Come),
    ("look", "Felt: look at this", Action::Look),
    ("repeat", "Felt: repeat the last line", Action::Repeat),
    (
        "stop",
        "Felt: stop / hush (again: talk again)",
        Action::Stop,
    ),
    ("clean", "Felt: clean house", Action::Clean),
    (
        "ledge",
        "Felt: go to the ledge under the cursor",
        Action::Ledge,
    ),
];

/// Binds the shortcuts and sends each activation to `tx` until the session ends.
pub async fn run(conn: Connection, tx: mpsc::UnboundedSender<Action>) -> Result<()> {
    register(&conn).await?;
    let session = create_session(&conn).await?;
    bind(&conn, &session).await?;
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.GlobalShortcuts")?
        .member("Activated")?
        .build();
    let mut activations = MessageStream::for_match_rule(rule, &conn, None).await?;
    tracing::info!("hotkeys: bound {} shortcuts", SHORTCUTS.len());
    while let Some(Ok(msg)) = activations.next().await {
        let Ok((path, id, _ts, _opts)) =
            msg.body()
                .deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>()
        else {
            continue;
        };
        if path != session {
            continue;
        }
        if let Some((_, _, action)) = SHORTCUTS.iter().find(|s| s.0 == id) {
            tracing::info!("hotkeys: {id}");
            let _ = tx.send(*action);
        }
    }
    Ok(())
}

/// A host (unsandboxed) app tells the portal who it is - before any other portal call on this connection.
async fn register(conn: &Connection) -> Result<()> {
    let opts: HashMap<&str, Value> = HashMap::new();
    conn.call_method(
        Some(PORTAL),
        PORTAL_PATH,
        Some("org.freedesktop.host.portal.Registry"),
        "Register",
        &(APP_ID, opts),
    )
    .await
    .context("portal Registry.Register")?;
    Ok(())
}

async fn create_session(conn: &Connection) -> Result<OwnedObjectPath> {
    let token = "felt_tray_session";
    let opts: HashMap<&str, Value> = HashMap::from([
        ("handle_token", Value::from("felt_tray_create")),
        ("session_handle_token", Value::from(token)),
    ]);
    let results = request(conn, "felt_tray_create", "CreateSession", &(opts,)).await?;
    let handle: String = results
        .get("session_handle")
        .and_then(|v| String::try_from(v.clone()).ok())
        .context("no session_handle in CreateSession's response")?;
    Ok(OwnedObjectPath::try_from(handle)?)
}

async fn bind(conn: &Connection, session: &OwnedObjectPath) -> Result<()> {
    let shortcuts: Vec<(&str, HashMap<&str, Value>)> = SHORTCUTS
        .iter()
        .map(|(id, desc, _)| (*id, HashMap::from([("description", Value::from(*desc))])))
        .collect();
    let opts: HashMap<&str, Value> =
        HashMap::from([("handle_token", Value::from("felt_tray_bind"))]);
    let session = ObjectPath::try_from(session.as_str())?;
    request(
        conn,
        "felt_tray_bind",
        "BindShortcuts",
        &(session, shortcuts, "", opts),
    )
    .await?;
    Ok(())
}

/// A portal call that answers through a Request object's Response signal: subscribe to that path first (it's
/// predictable from our unique name and the handle token), call, then wait for the answer.
async fn request<B>(
    conn: &Connection,
    token: &str,
    method: &str,
    body: &B,
) -> Result<HashMap<String, OwnedValue>>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    let sender = conn
        .unique_name()
        .context("no unique name")?
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("{PORTAL_PATH}/request/{sender}/{token}");
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")?
        .member("Response")?
        .path(path.as_str())?
        .build();
    let mut responses = MessageStream::for_match_rule(rule, conn, Some(1)).await?;
    conn.call_method(
        Some(PORTAL),
        PORTAL_PATH,
        Some("org.freedesktop.portal.GlobalShortcuts"),
        method,
        body,
    )
    .await
    .with_context(|| format!("portal GlobalShortcuts.{method}"))?;
    let msg = responses
        .next()
        .await
        .context("the portal closed the request")??;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
    if code != 0 {
        bail!(
            "portal GlobalShortcuts.{method} refused (response {code}) - cancelled in the dialog?"
        );
    }
    Ok(results)
}
