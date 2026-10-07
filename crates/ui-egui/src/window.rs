use egui::{Context, ViewportCommand};
use raw_window_handle::RawWindowHandle;

pub fn supports_hiding(handle: Option<RawWindowHandle>) -> bool {
    // winit's Wayland set_visible is a no-op. Never cancel a close request
    // unless the actual window backend can hide the window.
    matches!(
        handle,
        Some(
            RawWindowHandle::Xlib(_)
                | RawWindowHandle::Xcb(_)
                | RawWindowHandle::Win32(_)
                | RawWindowHandle::AppKit(_)
        )
    )
}

pub fn handle_close(ctx: &Context, hide_to_tray: bool) {
    if hide_to_tray && ctx.input(|input| input.viewport().close_requested()) {
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{RawInput, ViewportEvent, ViewportId};
    use raw_window_handle::{WaylandWindowHandle, XlibWindowHandle};
    use std::ptr::NonNull;

    fn close_output(handle: Option<RawWindowHandle>) -> Vec<ViewportCommand> {
        let ctx = Context::default();
        let mut input = RawInput::default();
        input
            .viewports
            .get_mut(&ViewportId::ROOT)
            .unwrap()
            .events
            .push(ViewportEvent::Close);
        let output = ctx.run_logic(&input, |ctx| handle_close(ctx, supports_hiding(handle)));
        output
            .viewport_commands
            .get(&ViewportId::ROOT)
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn wayland_close_is_never_cancelled_even_with_tray_enabled() {
        let handle = RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::dangling()));
        let commands = close_output(Some(handle));
        assert!(!commands.contains(&ViewportCommand::CancelClose));
        assert!(!commands.contains(&ViewportCommand::Visible(false)));
        assert!(!supports_hiding(None));
    }

    #[test]
    fn x11_close_still_hides_to_tray() {
        let commands = close_output(Some(RawWindowHandle::Xlib(XlibWindowHandle::new(1))));
        assert!(commands.contains(&ViewportCommand::CancelClose));
        assert!(commands.contains(&ViewportCommand::Visible(false)));
    }
}
