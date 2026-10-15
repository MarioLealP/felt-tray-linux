//! A small session-bus API so a desktop shell can be Felt's face without being Felt: `org.clockmost.Felt` at
//! `/org/clockmost/Felt` - `ShowChat()` and a `Status` property ("checking" / "online" / "offline"). The bar's
//! Felt chip uses it; with no such shell, the tray icon does the same job.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use tao::event_loop::EventLoopProxy;
use zbus::{connection, interface};

use crate::UiEvent;

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

struct FeltBus {
    proxy: Mutex<EventLoopProxy<UiEvent>>,
    status: Arc<SharedStatus>,
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

    #[zbus(property)]
    fn status(&self) -> String {
        self.status.text().into()
    }
}

pub async fn serve(
    proxy: EventLoopProxy<UiEvent>,
    status: Arc<SharedStatus>,
) -> zbus::Result<zbus::Connection> {
    let bus = FeltBus {
        proxy: Mutex::new(proxy),
        status,
    };
    connection::Builder::session()?
        .name("org.clockmost.Felt")?
        .serve_at("/org/clockmost/Felt", bus)?
        .build()
        .await
}
