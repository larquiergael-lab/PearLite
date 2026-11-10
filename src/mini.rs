//! Mini player: a small always-on-top window opened from the tray icon. It stays until its
//! hide button is pressed, and can be dragged anywhere. It shares the main WebView2
//! environment, so it adds a renderer for a tiny static page, not another browser.

use crate::UserEvent;
use crate::media::Track;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Theme, Window, WindowBuilder, WindowId};
use wry::{MemoryUsageLevel, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

pub struct Mini {
    window: Window,
    webview: WebView,
}

impl Mini {
    pub fn new(
        target: &EventLoopWindowTarget<UserEvent>,
        main: &WebView,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let window = WindowBuilder::new()
            .with_title("Pear Lite")
            .with_inner_size(LogicalSize::new(340.0, 150.0))
            .with_decorations(false)
            .with_undecorated_shadow(true)
            .with_resizable(false)
            .with_always_on_top(true)
            .with_skip_taskbar(true)
            .with_theme(Some(Theme::Dark))
            .with_visible(false)
            .build(target)?;
        crate::win::round_corners(window.hwnd());
        let webview = WebViewBuilder::new()
            .with_environment(main.environment())
            .with_html(include_str!("mini.html"))
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(UserEvent::Mini(req.into_body()));
            })
            .with_background_color((32, 32, 32, 255))
            .with_theme(wry::Theme::Dark)
            .build(&window)?;
        Ok(Self { window, webview })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    /// Shows it where it was last left, or the first time next to the tray icon (above a
    /// bottom taskbar, below a top one). Either way it's kept inside a monitor's work area,
    /// so an unplugged monitor or a moved taskbar can't strand it off screen.
    pub fn show(&mut self, icon: &tray_icon::Rect, saved: Option<(i32, i32)>) {
        let size = self.window.outer_size();
        let (w, h) = (size.width as i32, size.height as i32);
        let (x, y) = match saved {
            Some((x, y)) => {
                let (left, top, right, bottom) = crate::win::work_area(x + w / 2, y + h / 2);
                (x.min(right - w).max(left), y.min(bottom - h).max(top))
            }
            None => {
                let cx = (icon.position.x + icon.size.width as f64 / 2.0) as i32;
                let cy = (icon.position.y + icon.size.height as f64 / 2.0) as i32;
                let (left, top, right, bottom) = crate::win::work_area(cx, cy);
                let margin = (12.0 * self.window.scale_factor()) as i32;
                let x = (cx - w / 2).min(right - w - margin).max(left + margin);
                (x, if cy > (top + bottom) / 2 { bottom - h - margin } else { top + margin })
            }
        };
        self.window.set_outer_position(PhysicalPosition::new(x, y));
        self.window.set_visible(true);
        let _ = self.webview.set_visible(true);
        let _ = self.webview.set_memory_usage_level(MemoryUsageLevel::Normal);
        self.focus();
    }

    pub fn focus(&self) {
        self.window.set_focus();
        let _ = self.webview.focus();
    }

    pub fn hide(&mut self) {
        self.window.set_visible(false);
        let _ = self.webview.set_visible(false);
        let _ = self.webview.set_memory_usage_level(MemoryUsageLevel::Low);
    }

    /// Starts a native window move; called while the mouse button is still down on the page.
    pub fn drag(&self) {
        let _ = self.window.drag_window();
    }

    pub fn update(&self, track: &Track, playing: bool, position: f64, duration: f64) {
        if !self.window.is_visible() {
            return;
        }
        let state = serde_json::json!({
            "title": track.title,
            "artist": track.artist,
            "art": track.art,
            "playing": playing,
            "pos": position,
            "dur": duration,
        });
        let _ = self.webview.evaluate_script(&format!("window.mini && mini.update({state})"));
    }
}
