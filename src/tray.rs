//! The tray icon (StatusNotifierItem - any Linux panel shows it, not just ours): click = open the chat,
//! menu = status line, open chat, quit. Menu actions go to the window loop as [`UiEvent`]s.

use ksni::menu::{MenuItem, StandardItem};
use tao::event_loop::EventLoopProxy;

use crate::UiEvent;

const ICONS: [&[u8]; 2] = [
    include_bytes!("../assets/felt-32.png"),
    include_bytes!("../assets/felt-64.png"),
];

pub struct FeltTray {
    pub online: Option<bool>,
    pub proxy: EventLoopProxy<UiEvent>,
    icons: Vec<ksni::Icon>,
}

impl FeltTray {
    pub fn new(proxy: EventLoopProxy<UiEvent>) -> Self {
        Self {
            online: None,
            proxy,
            icons: ICONS.iter().filter_map(|b| argb_icon(b)).collect(),
        }
    }

    fn status(&self) -> &'static str {
        match self.online {
            None => "Felt: checking...",
            Some(true) => "Felt: online",
            Some(false) => "Felt: offline",
        }
    }

    fn send(&self, e: UiEvent) {
        let _ = self.proxy.send_event(e);
    }
}

impl ksni::Tray for FeltTray {
    fn id(&self) -> String {
        "felt-tray".into()
    }

    fn title(&self) -> String {
        "Felt".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.status().into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(UiEvent::ShowChat);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: self.status().into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open chat".into(),
                activate: Box::new(|t: &mut Self| t.send(UiEvent::ShowChat)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| t.send(UiEvent::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// PNG -> the SNI pixmap format (ARGB32, network byte order).
fn argb_icon(png_bytes: &[u8]) -> Option<ksni::Icon> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
        .read_info()
        .ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let data = buf[..info.buffer_size()]
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|[r, g, b, a]| [*a, *r, *g, *b])
        .collect();
    Some(ksni::Icon {
        width: info.width as i32,
        height: info.height as i32,
        data,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn both_icons_decode() {
        for b in super::ICONS {
            let i = super::argb_icon(b).expect("icon decodes");
            assert_eq!(i.data.len(), (i.width * i.height * 4) as usize);
        }
    }
}
