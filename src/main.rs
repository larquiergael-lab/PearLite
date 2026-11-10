#![windows_subsystem = "windows"]

//! Pear Lite: a lean YouTube Music desktop client. One small native process drives
//! the system WebView2 runtime instead of shipping a private Chromium + Node.js.

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// Appends a line to `%APPDATA%\PearLite\pear-lite.log` when started with `--log`.
macro_rules! log {
    ($($arg:tt)*) => {
        if let Some(f) = crate::LOG.get() {
            if let Ok(mut f) = f.lock() {
                let _ = writeln!(f, $($arg)*);
            }
        }
    };
}

mod config;
mod discord;
#[cfg(target_env = "gnu")]
mod loader;
mod media;
mod mini;
mod win;

use config::Config;
use discord::Discord;
use media::{Smtc, Track};
use mini::Mini;
use serde::Deserialize;
use souvlaki::{MediaControlEvent, SeekDirection};
use std::cell::{Cell, OnceCell};
use std::rc::Rc;
use std::time::Instant;
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::platform::windows::{EventLoopBuilderExtWindows, WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Icon, Theme, Window, WindowBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use win::RemoteCmd;
use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
use wry::{
    MemoryUsageLevel, NewWindowResponse, ScrollBarStyle, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows,
    WebViewExtWindows,
};

const APP_TITLE: &str = "Pear Lite";
static ICON_64: &[u8] = include_bytes!("../assets/icon64.rgba");
static TRAY_PLAYING: &[u8] = include_bytes!("../assets/tray32.rgba");
static TRAY_PAUSED: &[u8] = include_bytes!("../assets/tray-paused32.rgba");

enum UserEvent {
    Page(String),
    Remote(RemoteCmd),
    Media(MediaControlEvent),
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    /// Load a YouTube Music URL that the page tried to open in a new window.
    Navigate(String),
    /// A top-level navigation inside the app started.
    Navigating(String),
    /// A button press, drag start or "ready" from the mini player page.
    Mini(String),
}

/// Messages posted by `inject.js` through `window.ipc`.
#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum PageMsg {
    Meta { title: String, artist: String, album: String, art: String },
    State { playing: bool, pos: f64, dur: f64 },
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let remote = args.iter().find_map(|a| RemoteCmd::from_arg(a));
    let start_hidden = args.iter().any(|a| a == "--minimized");
    // `pear-lite.exe https://music.youtube.com/playlist?list=...` opens that page.
    let start_url = args.iter().find(|a| win::is_ytm_url(a)).cloned();

    let Some(_lock) = win::acquire_instance_lock() else {
        win::send_to_running_instance(remote.unwrap_or(RemoteCmd::Show));
        return;
    };
    if remote.is_some() {
        // `--next` etc. only make sense for a running instance; don't launch the app for them.
        return;
    }

    if args.iter().any(|a| a == "--log") {
        let _ = std::fs::create_dir_all(config::config_dir());
        if let Ok(f) = File::create(config::config_dir().join("pear-lite.log")) {
            let _ = LOG.set(Mutex::new(f));
        }
    }

    if let Err(e) = run(start_hidden, start_url) {
        log!("fatal: {e}");
        win::error_box(&format!(
            "Pear Lite could not start:\n\n{e}\n\nIt needs the Microsoft Edge WebView2 Runtime (preinstalled on Windows 11):\nhttps://go.microsoft.com/fwlink/p/?LinkId=2124703"
        ));
    }
}

fn run(start_hidden: bool, start_url: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::load();

    // Peek at posted WM_CLOSE before dispatch. The close button *sends* it; only taskkill and
    // similar tools *post* it. Honour that as "quit" for the main window, and never let it
    // destroy tao's or the tray's hidden helper windows, which would leave the app deaf.
    let proxy_cell: Rc<OnceCell<EventLoopProxy<UserEvent>>> = Rc::default();
    let main_hwnd: Rc<Cell<isize>> = Rc::default();
    let hook_cell = proxy_cell.clone();
    let hook_hwnd = main_hwnd.clone();
    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event()
        .with_msg_hook(move |msg| {
            let msg = unsafe { &*(msg as *const windows::Win32::UI::WindowsAndMessaging::MSG) };
            if msg.message != WM_CLOSE || !win::is_top_level(msg.hwnd) {
                return false;
            }
            log!("posted WM_CLOSE");
            if let (Some(proxy), true) = (hook_cell.get(), msg.hwnd.0 as isize == hook_hwnd.get()) {
                let _ = proxy.send_event(UserEvent::Remote(RemoteCmd::Quit));
            }
            true
        })
        .build();
    let proxy = event_loop.create_proxy();
    let _ = proxy_cell.set(proxy.clone());

    let window = build_window(&event_loop, &cfg)?;
    main_hwnd.set(window.hwnd());
    let remote_proxy = proxy.clone();
    win::listen_for_remote_commands(
        window.hwnd(),
        Box::new(move |cmd| {
            log!("remote message: {cmd:?}");
            let _ = remote_proxy.send_event(UserEvent::Remote(cmd));
        }),
    );

    let mut web_context = WebContext::new(Some(config::data_dir()));
    let ipc_proxy = proxy.clone();
    let nav_proxy = proxy.clone();
    let new_window_proxy = proxy.clone();
    let webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_url(start_url.unwrap_or_else(|| cfg.start_url()))
        .with_initialization_script_for_main_only(init_script(&cfg), true)
        .with_ipc_handler(move |req| {
            let _ = ipc_proxy.send_event(UserEvent::Page(req.into_body()));
        })
        .with_navigation_handler(move |url| {
            if win::is_in_app_url(&url) {
                let _ = nav_proxy.send_event(UserEvent::Navigating(url));
                return true;
            }
            log!("external: {url}");
            win::open_external(&url);
            false
        })
        .with_new_window_req_handler(move |url, _| {
            if win::is_ytm_url(&url) {
                let _ = new_window_proxy.send_event(UserEvent::Navigate(url));
            } else {
                win::open_external(&url);
            }
            NewWindowResponse::Deny
        })
        .with_background_color((3, 3, 3, 255))
        .with_devtools(cfg.devtools)
        .with_focused(!start_hidden)
        .with_additional_browser_args(browser_args(&cfg))
        .with_theme(wry::Theme::Dark)
        .with_scroll_bar_style(ScrollBarStyle::FluentOverlay)
        .build(&window)?;

    if cfg.ad_block {
        if let Err(e) = win::install_request_blocker(&webview) {
            log!("request blocker: {e}");
        }
    }

    let media_proxy = proxy.clone();
    let smtc = Smtc::new(window.hwnd(), move |ev| {
        let _ = media_proxy.send_event(UserEvent::Media(ev));
    })
    .map_err(|e| log!("media controls: {e:?}"))
    .ok();

    let (tray, menu) = build_tray(&cfg, &proxy)?;
    let discord = cfg.discord_rpc.then(|| Discord::spawn(cfg.discord_client_id.clone()));

    let mut app = App {
        cfg,
        window,
        webview,
        _web_context: web_context,
        proxy,
        mini: None,
        tray,
        menu,
        smtc,
        discord,
        track: Track::default(),
        playing: false,
        position: 0.0,
        position_at: Instant::now(),
        duration: 0.0,
        webview_visible: true,
    };
    if start_hidden {
        app.hide();
    } else {
        app.show();
    }

    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let quit = match event {
            Event::WindowEvent { window_id, event, .. } if app.mini.as_ref().is_some_and(|m| m.id() == window_id) => {
                app.on_mini_window(event);
                false
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                if app.cfg.close_to_tray {
                    app.hide();
                    false
                } else {
                    true
                }
            }
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                app.on_resized(size);
                false
            }
            Event::WindowEvent { event: WindowEvent::Moved(_), .. } => {
                app.remember_geometry();
                false
            }
            Event::UserEvent(ev) => app.on_user_event(ev, target),
            _ => false,
        };
        if quit {
            log!("quitting");
            app.save_state();
            *control_flow = ControlFlow::Exit;
        }
    });
}

fn build_window(event_loop: &EventLoop<UserEvent>, cfg: &Config) -> Result<Window, tao::error::OsError> {
    let icon = Icon::from_rgba(ICON_64.to_vec(), 64, 64).ok();
    let ws = &cfg.window;
    let mut builder = WindowBuilder::new()
        .with_title(APP_TITLE)
        .with_window_icon(icon.clone())
        .with_taskbar_icon(icon)
        .with_window_classname(win::WINDOW_CLASS)
        .with_theme(Some(Theme::Dark))
        .with_inner_size(PhysicalSize::new(ws.width.max(400), ws.height.max(300)))
        .with_min_inner_size(LogicalSize::new(360.0, 300.0))
        .with_maximized(ws.maximized)
        .with_visible(false);
    if let (Some(x), Some(y)) = (ws.x, ws.y) {
        // Only restore the position if the title bar would still be reachable on some monitor.
        let visible = event_loop.available_monitors().any(|m| {
            let (p, s) = (m.position(), m.size());
            x + 100 < p.x + s.width as i32 && x + ws.width as i32 - 100 > p.x && y >= p.y - 8 && y + 40 < p.y + s.height as i32
        });
        if visible {
            builder = builder.with_position(PhysicalPosition::new(x, y));
        }
    }
    builder.build(event_loop)
}

fn browser_args(cfg: &Config) -> String {
    // wry's defaults (mini menu / SmartScreen off) plus: Chromium's own media-key/SMTC
    // handling off, since we drive SMTC ourselves and two sessions would fight.
    let mut args = String::from(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,HardwareMediaKeyHandling \
         --autoplay-policy=no-user-gesture-required",
    );
    if !cfg.hardware_acceleration {
        args.push_str(" --disable-gpu --disable-gpu-compositing");
    }
    if !cfg.extra_browser_args.trim().is_empty() {
        args.push(' ');
        args.push_str(cfg.extra_browser_args.trim());
    }
    args
}

fn init_script(cfg: &Config) -> String {
    let dir = config::config_dir();
    let css = std::fs::read_to_string(dir.join("custom.css")).unwrap_or_default();
    let flags = serde_json::json!({ "adBlock": cfg.ad_block, "css": css });
    let mut script = format!("window.__PEAR_LITE_CFG__ = {flags};\n{}", include_str!("inject.js"));
    if let Ok(js) = std::fs::read_to_string(dir.join("custom.js")) {
        script.push_str(&format!(
            "\n;(() => {{ if (location.hostname !== 'music.youtube.com') return;\ntry {{\n{js}\n}} catch (e) {{ console.error('custom.js', e); }}\n}})();"
        ));
    }
    script
}

fn tray_image(rgba: &[u8]) -> Option<tray_icon::Icon> {
    tray_icon::Icon::from_rgba(rgba.to_vec(), 32, 32).ok()
}

struct TrayMenu {
    now_playing: MenuItem,
    show_hide: MenuItem,
    play_pause: MenuItem,
    next: MenuItem,
    previous: MenuItem,
    close_to_tray: CheckMenuItem,
    ad_block: CheckMenuItem,
    discord: CheckMenuItem,
    low_memory: CheckMenuItem,
    gpu: CheckMenuItem,
    autostart: CheckMenuItem,
    reload: MenuItem,
    settings: MenuItem,
    quit: MenuItem,
}

fn build_tray(cfg: &Config, proxy: &EventLoopProxy<UserEvent>) -> Result<(Option<TrayIcon>, TrayMenu), tray_icon::menu::Error> {
    let m = TrayMenu {
        now_playing: MenuItem::new("Nothing playing", false, None),
        show_hide: MenuItem::new("Show / Hide window", true, None),
        play_pause: MenuItem::new("Play / Pause", true, None),
        next: MenuItem::new("Next", true, None),
        previous: MenuItem::new("Previous", true, None),
        close_to_tray: CheckMenuItem::new("Close to tray", true, cfg.close_to_tray, None),
        ad_block: CheckMenuItem::new("Block ads (applies on restart)", true, cfg.ad_block, None),
        discord: CheckMenuItem::new("Discord Rich Presence", true, cfg.discord_rpc, None),
        low_memory: CheckMenuItem::new("Trim memory when hidden", true, cfg.low_memory_when_hidden, None),
        gpu: CheckMenuItem::new("GPU acceleration (applies on restart)", true, cfg.hardware_acceleration, None),
        autostart: CheckMenuItem::new("Start with Windows", true, win::autostart_enabled(), None),
        reload: MenuItem::new("Reload page", true, None),
        settings: MenuItem::new("Open settings folder", true, None),
        quit: MenuItem::new("Quit", true, None),
    };
    let menu = Menu::new();
    let sep = || PredefinedMenuItem::separator();
    menu.append_items(&[
        &m.now_playing,
        &sep(),
        &m.play_pause,
        &m.next,
        &m.previous,
        &sep(),
        &m.show_hide,
        &m.close_to_tray,
        &m.ad_block,
        &m.discord,
        &m.low_memory,
        &m.gpu,
        &m.autostart,
        &sep(),
        &m.reload,
        &m.settings,
        &m.quit,
    ])?;

    let p = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Tray(e));
    }));
    let p = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Menu(e));
    }));

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(APP_TITLE);
    if let Some(icon) = tray_image(TRAY_PAUSED) {
        builder = builder.with_icon(icon);
    }
    // A missing tray (e.g. Explorer not running) shouldn't prevent playback.
    let tray = builder.build().map_err(|e| log!("tray: {e}")).ok();
    Ok((tray, m))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

struct App {
    cfg: Config,
    window: Window,
    webview: WebView,
    _web_context: WebContext,
    proxy: EventLoopProxy<UserEvent>,
    /// Created on first use, then kept so it opens instantly.
    mini: Option<Mini>,
    tray: Option<TrayIcon>,
    menu: TrayMenu,
    smtc: Option<Smtc>,
    discord: Option<Discord>,
    track: Track,
    playing: bool,
    /// Last reported position and when it was reported (to extrapolate while playing).
    position: f64,
    position_at: Instant,
    duration: f64,
    webview_visible: bool,
}

#[derive(Clone, Copy)]
enum MenuAction {
    ShowHide,
    PlayPause,
    Next,
    Previous,
    CloseToTray,
    AdBlock,
    Discord,
    LowMemory,
    Gpu,
    Autostart,
    Reload,
    Settings,
    Quit,
}

impl App {
    fn on_user_event(&mut self, ev: UserEvent, target: &EventLoopWindowTarget<UserEvent>) -> bool {
        match ev {
            UserEvent::Page(body) => self.on_page(&body),
            UserEvent::Navigate(url) => {
                let _ = self.webview.load_url(&url);
            }
            UserEvent::Navigating(url) => {
                // Dark canvas avoids a white flash in YouTube Music; Google's sign-in and
                // consent pages assume the default white canvas behind their dark text.
                let color = if win::is_ytm_url(&url) { (3, 3, 3, 255) } else { (255, 255, 255, 255) };
                let _ = self.webview.set_background_color(color);
            }
            UserEvent::Media(ev) => return self.on_media(ev),
            UserEvent::Remote(cmd) => {
                log!("remote command: {cmd:?}");
                match cmd {
                    RemoteCmd::Show => self.show(),
                    RemoteCmd::PlayPause => self.page_cmd("toggle", 0.0),
                    RemoteCmd::Next => self.page_cmd("next", 0.0),
                    RemoteCmd::Previous => self.page_cmd("prev", 0.0),
                    RemoteCmd::Quit => return true,
                }
            }
            UserEvent::Tray(TrayIconEvent::Click {
                button: MouseButton::Left, button_state: MouseButtonState::Up, rect, ..
            }) => self.open_mini(target, &rect),
            UserEvent::Tray(_) => {}
            UserEvent::Menu(ev) => return self.on_menu(&ev.id),
            UserEvent::Mini(msg) => self.on_mini(&msg),
        }
        false
    }

    /// Only the mini player's own hide button closes it; clicking the tray icon again just
    /// brings it to the front.
    fn open_mini(&mut self, target: &EventLoopWindowTarget<UserEvent>, icon: &tray_icon::Rect) {
        if let Some(m) = self.mini.as_ref().filter(|m| m.is_visible()) {
            return m.focus();
        }
        if self.mini.is_none() {
            match Mini::new(target, &self.webview, self.proxy.clone()) {
                Ok(m) => self.mini = Some(m),
                Err(e) => {
                    log!("mini player: {e}");
                    return self.toggle_window();
                }
            }
        }
        if let Some(m) = &mut self.mini {
            m.show(icon, self.cfg.mini_position);
        }
        self.update_mini();
    }

    fn on_mini_window(&mut self, event: WindowEvent) {
        match event {
            WindowEvent::Moved(p) => self.cfg.mini_position = Some((p.x, p.y)),
            WindowEvent::CloseRequested => self.hide_mini(),
            _ => {}
        }
    }

    fn on_mini(&mut self, msg: &str) {
        match msg {
            "ready" => self.update_mini(),
            "drag" => {
                if let Some(m) = &self.mini {
                    m.drag();
                }
            }
            "toggle" => self.page_cmd("toggle", 0.0),
            "prev" => self.page_cmd("prev", 0.0),
            "next" => self.page_cmd("next", 0.0),
            "back" => self.seek_by(-10.0),
            "forward" => self.seek_by(10.0),
            "open" => self.show(),
            "hide" => self.hide_mini(),
            _ => {
                if let Some(t) = msg.strip_prefix("seek:").and_then(|s| s.parse::<f64>().ok()).filter(|t| t.is_finite()) {
                    self.page_cmd("seek", t.max(0.0));
                }
            }
        }
    }

    fn hide_mini(&mut self) {
        if let Some(m) = &mut self.mini {
            m.hide();
            // Persist the spot it was dragged to now, not only on a clean quit.
            self.cfg.save();
        }
    }

    fn update_mini(&self) {
        if let Some(m) = &self.mini {
            m.update(&self.track, self.playing, self.current_position(), self.duration);
        }
    }

    fn page_cmd(&self, cmd: &str, arg: f64) {
        let _ = self.webview.evaluate_script(&format!("window.__pearLite && window.__pearLite.cmd('{cmd}', {arg})"));
    }

    fn current_position(&self) -> f64 {
        self.position + if self.playing { self.position_at.elapsed().as_secs_f64() } else { 0.0 }
    }

    fn show(&mut self) {
        self.window.set_visible(true);
        if self.window.is_minimized() {
            self.window.set_minimized(false);
        }
        self.window.set_focus();
        self.set_webview_visible(true);
    }

    fn hide(&mut self) {
        self.window.set_visible(false);
        self.set_webview_visible(false);
    }

    fn toggle_window(&mut self) {
        if self.window.is_visible() && !self.window.is_minimized() {
            self.hide();
        } else {
            self.show();
        }
    }

    /// A hidden WebView2 stops rendering (audio keeps playing); "Low" additionally lets
    /// it drop caches and trim its working set while nobody is looking.
    fn set_webview_visible(&mut self, visible: bool) {
        if self.webview_visible == visible {
            return;
        }
        self.webview_visible = visible;
        let _ = self.webview.set_visible(visible);
        if visible || self.cfg.low_memory_when_hidden {
            let level = if visible { MemoryUsageLevel::Normal } else { MemoryUsageLevel::Low };
            let _ = self.webview.set_memory_usage_level(level);
        }
        log!("webview visible: {visible}");
    }

    /// Windows reports a 0x0 size when minimizing. Decide from the event itself rather than
    /// querying the window: events raised during our own handlers are delivered late, and the
    /// live state can disagree with them by then.
    fn on_resized(&mut self, size: PhysicalSize<u32>) {
        let minimized = size.width == 0 && size.height == 0;
        if self.window.is_visible() {
            self.set_webview_visible(!minimized);
        }
        if !minimized {
            self.remember_geometry();
        }
    }

    fn remember_geometry(&mut self) {
        if self.window.is_minimized() || !self.window.is_visible() {
            return;
        }
        let ws = &mut self.cfg.window;
        ws.maximized = self.window.is_maximized();
        if !ws.maximized {
            if let Ok(p) = self.window.outer_position() {
                ws.x = Some(p.x);
                ws.y = Some(p.y);
            }
            let s = self.window.inner_size();
            if s.width > 0 && s.height > 0 {
                ws.width = s.width;
                ws.height = s.height;
            }
        }
    }

    fn on_page(&mut self, body: &str) {
        let Ok(msg) = serde_json::from_str::<PageMsg>(body) else {
            log!("unexpected ipc: {body}");
            return;
        };
        match msg {
            PageMsg::Meta { title, artist, album, art } => {
                let track = Track { title, artist, album, art };
                if track == self.track {
                    return;
                }
                log!("track: {} - {} [{}]", track.title, track.artist, track.album);
                self.track = track;
                self.duration = 0.0;
                if let Some(s) = &mut self.smtc {
                    s.set_track(&self.track, 0.0);
                }
                let label = format!("{} — {}", self.track.title, self.track.artist);
                self.window.set_title(&format!("{label} · {APP_TITLE}"));
                if let Some(t) = &self.tray {
                    let _ = t.set_tooltip(Some(truncate(&label, 120)));
                }
                self.menu.now_playing.set_text(truncate(&label, 60));
                self.update_discord();
                self.update_mini();
            }
            PageMsg::State { playing, pos, dur } => {
                let seeked = (pos - self.current_position()).abs() > 3.0;
                let playing_changed = playing != self.playing;
                let duration_changed = (dur - self.duration).abs() > 1.0;
                self.playing = playing;
                self.position = pos;
                self.position_at = Instant::now();
                if duration_changed {
                    self.duration = dur;
                }
                if let Some(s) = &mut self.smtc {
                    if duration_changed {
                        s.set_track(&self.track, dur);
                    }
                    s.set_playback(playing, pos);
                }
                if playing_changed {
                    log!("playing: {playing} at {pos:.1}/{dur:.1}");
                    if let Some(t) = &self.tray {
                        let _ = t.set_icon(tray_image(if playing { TRAY_PLAYING } else { TRAY_PAUSED }));
                    }
                }
                if playing_changed || seeked || duration_changed {
                    self.update_discord();
                }
                self.update_mini();
            }
        }
    }

    fn update_discord(&self) {
        let Some(d) = &self.discord else { return };
        if self.playing && !self.track.title.is_empty() {
            d.playing(&self.track, self.position, self.duration);
        } else {
            d.clear();
        }
    }

    fn on_media(&mut self, ev: MediaControlEvent) -> bool {
        log!("media key: {ev:?}");
        match ev {
            MediaControlEvent::Play => self.page_cmd("play", 0.0),
            MediaControlEvent::Pause | MediaControlEvent::Stop => self.page_cmd("pause", 0.0),
            MediaControlEvent::Toggle => self.page_cmd("toggle", 0.0),
            MediaControlEvent::Next => self.page_cmd("next", 0.0),
            MediaControlEvent::Previous => self.page_cmd("prev", 0.0),
            MediaControlEvent::SetPosition(p) => self.page_cmd("seek", p.0.as_secs_f64()),
            MediaControlEvent::Seek(dir) => self.seek_by(if dir == SeekDirection::Forward { 10.0 } else { -10.0 }),
            MediaControlEvent::SeekBy(dir, d) => {
                let d = d.as_secs_f64();
                self.seek_by(if dir == SeekDirection::Forward { d } else { -d })
            }
            MediaControlEvent::Raise => self.show(),
            MediaControlEvent::Quit => return true,
            _ => {}
        }
        false
    }

    fn seek_by(&self, delta: f64) {
        self.page_cmd("seek", (self.current_position() + delta).max(0.0));
    }

    fn menu_action(&self, id: &MenuId) -> Option<MenuAction> {
        let m = &self.menu;
        let items: [(&MenuId, MenuAction); 13] = [
            (m.show_hide.id(), MenuAction::ShowHide),
            (m.play_pause.id(), MenuAction::PlayPause),
            (m.next.id(), MenuAction::Next),
            (m.previous.id(), MenuAction::Previous),
            (m.close_to_tray.id(), MenuAction::CloseToTray),
            (m.ad_block.id(), MenuAction::AdBlock),
            (m.discord.id(), MenuAction::Discord),
            (m.low_memory.id(), MenuAction::LowMemory),
            (m.gpu.id(), MenuAction::Gpu),
            (m.autostart.id(), MenuAction::Autostart),
            (m.reload.id(), MenuAction::Reload),
            (m.settings.id(), MenuAction::Settings),
            (m.quit.id(), MenuAction::Quit),
        ];
        items.iter().find(|(item, _)| *item == id).map(|(_, a)| *a)
    }

    fn on_menu(&mut self, id: &MenuId) -> bool {
        let Some(action) = self.menu_action(id) else { return false };
        match action {
            MenuAction::ShowHide => self.toggle_window(),
            MenuAction::PlayPause => self.page_cmd("toggle", 0.0),
            MenuAction::Next => self.page_cmd("next", 0.0),
            MenuAction::Previous => self.page_cmd("prev", 0.0),
            MenuAction::CloseToTray => {
                self.cfg.close_to_tray = self.menu.close_to_tray.is_checked();
                self.cfg.save();
            }
            MenuAction::AdBlock => {
                self.cfg.ad_block = self.menu.ad_block.is_checked();
                self.cfg.save();
            }
            MenuAction::Discord => {
                self.cfg.discord_rpc = self.menu.discord.is_checked();
                // Dropping the handle ends the thread, which clears the activity.
                self.discord = self.cfg.discord_rpc.then(|| Discord::spawn(self.cfg.discord_client_id.clone()));
                self.update_discord();
                self.cfg.save();
            }
            MenuAction::LowMemory => {
                self.cfg.low_memory_when_hidden = self.menu.low_memory.is_checked();
                self.cfg.save();
            }
            MenuAction::Gpu => {
                self.cfg.hardware_acceleration = self.menu.gpu.is_checked();
                self.cfg.save();
            }
            MenuAction::Autostart => {
                win::set_autostart(self.menu.autostart.is_checked());
                self.menu.autostart.set_checked(win::autostart_enabled());
            }
            MenuAction::Reload => {
                let _ = self.webview.reload();
            }
            MenuAction::Settings => {
                self.cfg.save();
                win::open_folder(&config::config_dir());
            }
            MenuAction::Quit => return true,
        }
        false
    }

    fn save_state(&mut self) {
        if let Ok(url) = self.webview.url() {
            if win::is_ytm_url(&url) {
                self.cfg.last_url = Some(url);
            }
        }
        self.cfg.save();
    }
}
