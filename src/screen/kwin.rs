//! KWin: our window-list script (loaded through KWin's scripting D-Bus API) and per-window screenshots
//! (`org.kde.KWin.ScreenShot2.CaptureWindow`, granted to /usr/bin/felt-tray by its .desktop file's
//! `X-KDE-DBUS-Restricted-Interfaces`). KDE Plasma only.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use image::RgbImage;
use serde::Deserialize;
use zbus::Connection;
use zbus::zvariant::{Fd, OwnedValue, Value};

use super::policy::WindowInfo;

/// Our own plugin name - never the bar's (`clockmost-bridge`).
const PLUGIN: &str = "felt-tray-windows";
const SCRIPT: &str = include_str!("kwin_script.js");

/// What the script sends on every change.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    pub active: String,
    pub windows: Vec<WindowInfo>,
}

async fn scripting<B>(conn: &Connection, method: &str, body: &B) -> Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(
        Some("org.kde.KWin"),
        "/Scripting",
        Some("org.kde.kwin.Scripting"),
        method,
        body,
    )
    .await
    .with_context(|| format!("KWin Scripting.{method}"))
}

/// Replace any copy left by an earlier run, then load and start ours.
pub async fn load_script(conn: &Connection) -> Result<()> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    let path = PathBuf::from(dir).join("felt-tray-kwin.js");
    std::fs::write(&path, SCRIPT).with_context(|| format!("writing {}", path.display()))?;
    unload_script(conn).await?;
    let path = path.to_str().context("script path is not UTF-8")?;
    let id: i32 = scripting(conn, "loadScript", &(path, PLUGIN))
        .await?
        .body()
        .deserialize()?;
    anyhow::ensure!(id >= 0, "KWin refused the script (id {id})");
    scripting(conn, "start", &()).await?;
    Ok(())
}

pub async fn unload_script(conn: &Connection) -> Result<()> {
    scripting(conn, "unloadScript", &(PLUGIN,)).await?;
    Ok(())
}

/// Captures ONE window (no decoration, no cursor) - call only with a window the policy chose.
pub async fn capture_window(conn: &Connection, id: &str) -> Result<RgbImage> {
    let (mut reader, writer) = std::io::pipe()?;
    let writer: OwnedFd = writer.into();
    // KWin writes after it replies; read concurrently so a full pipe never stalls it
    let read = tokio::task::spawn_blocking(move || {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).map(|_| buf)
    });
    let opts: HashMap<&str, Value<'_>> = HashMap::from([
        ("include-decoration", Value::from(false)),
        ("include-shadow", Value::from(false)),
        ("include-cursor", Value::from(false)),
    ]);
    let reply = conn
        .call_method(
            Some("org.kde.KWin"),
            "/org/kde/KWin/ScreenShot2",
            Some("org.kde.KWin.ScreenShot2"),
            "CaptureWindow",
            &(id, opts, Fd::from(&writer)),
        )
        .await;
    drop(writer); // our end closed: the read sees EOF once KWin is done
    let meta: HashMap<String, OwnedValue> = reply
        .context("KWin ScreenShot2.CaptureWindow (is felt-tray's .desktop grant installed?)")?
        .body()
        .deserialize()?;
    let data = read.await??;
    let get = |k: &str| -> Result<u32> {
        let v = meta
            .get(k)
            .with_context(|| format!("no {k} in the screenshot reply"))?;
        Ok(u32::try_from(v)?)
    };
    to_rgb(
        &data,
        get("width")?,
        get("height")?,
        get("stride")?,
        get("format")?,
    )
}

/// KWin hands back a raw QImage buffer; 32-bit formats only.
fn to_rgb(data: &[u8], width: u32, height: u32, stride: u32, format: u32) -> Result<RgbImage> {
    // QImage::Format: 4 RGB32, 5 ARGB32, 6 ARGB32_Premultiplied (BGRA in memory, little endian);
    // 16 RGBX8888, 17 RGBA8888, 18 RGBA8888_Premultiplied (RGBA in memory)
    let bgra = match format {
        4..=6 => true,
        16..=18 => false,
        f => bail!("unsupported screenshot format {f}"),
    };
    let (w, h, stride) = (width as usize, height as usize, stride as usize);
    anyhow::ensure!(
        stride >= w * 4 && data.len() >= stride * h,
        "short screenshot buffer"
    );
    let mut out = RgbImage::new(width, height);
    for (y, row) in data.chunks(stride).take(h).enumerate() {
        for (x, px) in row[..w * 4].as_chunks::<4>().0.iter().enumerate() {
            let rgb = if bgra {
                [px[2], px[1], px[0]]
            } else {
                [px[0], px[1], px[2]]
            };
            out.put_pixel(x as u32, y as u32, image::Rgb(rgb));
        }
    }
    Ok(out)
}

/// Seconds since the last input (KDE's ScreenSaver idle time); None if unknown.
pub async fn idle_seconds(conn: &Connection) -> Option<u64> {
    let ms: u32 = conn
        .call_method(
            Some("org.freedesktop.ScreenSaver"),
            "/ScreenSaver",
            Some("org.freedesktop.ScreenSaver"),
            "GetSessionIdleTime",
            &(),
        )
        .await
        .ok()?
        .body()
        .deserialize()
        .ok()?;
    Some(u64::from(ms) / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_both_byte_orders_and_honours_stride() {
        // 2x1, stride 12 (4 bytes padding)
        let bgra = [10, 20, 30, 255, 1, 2, 3, 255, 0, 0, 0, 0];
        let img = to_rgb(&bgra, 2, 1, 12, 6).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [30, 20, 10]);
        let img = to_rgb(&bgra, 2, 1, 12, 17).unwrap();
        assert_eq!(img.get_pixel(1, 0).0, [1, 2, 3]);
        assert!(to_rgb(&bgra, 2, 1, 12, 13).is_err());
        assert!(to_rgb(&bgra[..4], 2, 1, 12, 6).is_err());
    }
}
