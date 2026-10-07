use crate::assets::icon_rgba;
use egui::Context;
use std::sync::mpsc::{self, Receiver};
use tray_icon::{
    Icon, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

pub enum TrayAction {
    Show,
    Refresh,
    Quit,
}
pub struct Tray {
    _icon: TrayIcon,
    events: Receiver<TrayAction>,
}
impl Tray {
    pub fn new(ctx: &Context) -> Result<Self, String> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let connection = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
            let proxy =
                zbus::blocking::fdo::DBusProxy::new(&connection).map_err(|e| e.to_string())?;
            let name = zbus::names::BusName::try_from("org.kde.StatusNotifierWatcher")
                .map_err(|e| e.to_string())?;
            if !proxy.name_has_owner(name).map_err(|e| e.to_string())? {
                return Err("No system tray watcher is running.".into());
            }
        }
        let menu = Menu::new();
        let show = MenuItem::new("Open CraftLauncher", true, None);
        let refresh = MenuItem::new("Check for updates", true, None);
        let quit = MenuItem::new("Quit CraftLauncher", true, None);
        menu.append(&show).map_err(|e| e.to_string())?;
        menu.append(&refresh).map_err(|e| e.to_string())?;
        menu.append(&quit).map_err(|e| e.to_string())?;
        let icon = Icon::from_rgba(icon_rgba(64), 64, 64).map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("CraftLauncher — your creative toolbox")
            .with_menu(Box::new(menu))
            .build()
            .map_err(|e| e.to_string())?;
        let repaint = ctx.clone();
        let (send, events) = mpsc::channel();
        let clicks = send.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick { .. }
            ) {
                let _ = clicks.send(TrayAction::Show);
                repaint.request_repaint();
            }
        }));
        let repaint = ctx.clone();
        let show = show.id().clone();
        let refresh = refresh.id().clone();
        let quit = quit.id().clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == show {
                Some(TrayAction::Show)
            } else if event.id == refresh {
                Some(TrayAction::Refresh)
            } else if event.id == quit {
                Some(TrayAction::Quit)
            } else {
                None
            };
            if let Some(action) = action {
                let _ = send.send(action);
                repaint.request_repaint();
            }
        }));
        Ok(Self {
            _icon: icon,
            events,
        })
    }
    pub fn poll(&self) -> Option<TrayAction> {
        self.events.try_recv().ok()
    }
}
