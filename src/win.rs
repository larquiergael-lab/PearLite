//! Small Win32 helpers: single instance, remote commands, external links,
//! autostart and network-level ad blocking.

use webview2_com::{Microsoft::Web::WebView2::Win32::*, WebResourceRequestedEventHandler, take_pwstr};
use std::sync::OnceLock;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GWL_STYLE, GetWindowLongPtrW, GetWindowThreadProcessId, MB_ICONERROR, MB_OK,
    MessageBoxW, PostMessageW, RegisterWindowMessageW, SW_SHOWNORMAL, WS_CHILD,
};
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR, w};
use wry::WebViewExtWindows;

pub const WINDOW_CLASS: &str = "PearLiteWindow";
const APP_NAME: PCWSTR = w!("PearLite");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

/// Commands a second launch (`pear-lite.exe --next`, ...) forwards to the running instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteCmd {
    Show = 1,
    PlayPause = 2,
    Next = 3,
    Previous = 4,
    Quit = 5,
}

impl RemoteCmd {
    pub fn from_arg(arg: &str) -> Option<Self> {
        Some(match arg {
            "--play-pause" => Self::PlayPause,
            "--next" => Self::Next,
            "--previous" | "--prev" => Self::Previous,
            "--quit" => Self::Quit,
            _ => return None,
        })
    }

    pub fn from_wparam(v: usize) -> Option<Self> {
        Some(match v {
            1 => Self::Show,
            2 => Self::PlayPause,
            3 => Self::Next,
            4 => Self::Previous,
            5 => Self::Quit,
            _ => return None,
        })
    }
}

/// Window message used to deliver [`RemoteCmd`]s between instances.
pub fn remote_msg_id() -> u32 {
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| unsafe { RegisterWindowMessageW(w!("PearLite.RemoteCommand")) })
}

/// Receives [`RemoteCmd`]s in the window procedure. A message-loop hook would miss commands
/// that a nested loop (COM calls into WebView2 or the media service, menus, window dragging)
/// dispatches directly.
pub fn listen_for_remote_commands(hwnd: isize, on_cmd: Box<dyn Fn(RemoteCmd)>) {
    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
        if msg == remote_msg_id() {
            if let Some(cmd) = RemoteCmd::from_wparam(wp.0) {
                unsafe { (*(data as *const Box<dyn Fn(RemoteCmd)>))(cmd) };
            }
            return LRESULT(0);
        }
        unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
    }
    // Leaked on purpose: the callback must live as long as the window, i.e. the process.
    let data = Box::into_raw(Box::new(on_cmd)) as usize;
    unsafe {
        let _ = SetWindowSubclass(HWND(hwnd as _), Some(proc), 1, data);
    }
}

/// Holds the single-instance mutex for the lifetime of the process.
pub struct InstanceLock(HANDLE);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Returns `None` if another instance already owns the lock.
pub fn acquire_instance_lock() -> Option<InstanceLock> {
    unsafe {
        match CreateMutexW(None, true, w!("Local\\PearLite.SingleInstance")) {
            Ok(h) if GetLastError() == ERROR_ALREADY_EXISTS => {
                let _ = CloseHandle(h);
                None
            }
            Ok(h) => Some(InstanceLock(h)),
            // Can't create the mutex at all: run anyway rather than refuse to start.
            Err(_) => Some(InstanceLock(HANDLE::default())),
        }
    }
}

/// Sends `cmd` to the running instance's window. Retries briefly in case it is still starting.
pub fn send_to_running_instance(cmd: RemoteCmd) -> bool {
    let class = HSTRING::from(WINDOW_CLASS);
    for _ in 0..30 {
        unsafe {
            if let Ok(hwnd) = FindWindowW(&class, PCWSTR::null()) {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                // Let the running instance bring itself to the foreground.
                let _ = AllowSetForegroundWindow(pid);
                return PostMessageW(Some(hwnd), remote_msg_id(), WPARAM(cmd as usize), LPARAM(0)).is_ok();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

/// Opens an http(s) URL in the user's default browser. Other schemes are ignored on purpose.
pub fn open_external(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn is_top_level(hwnd: HWND) -> bool {
    unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CHILD.0 == 0 }
}

/// Screen minus taskbar of the monitor nearest to a point, as (left, top, right, bottom).
pub fn work_area(x: i32, y: i32) -> (i32, i32, i32, i32) {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let r = info.rcWork;
        (r.left, r.top, r.right, r.bottom)
    }
}

/// Windows 11 rounds decorated windows only; ask for it on borderless ones too (no-op on Windows 10).
pub fn round_corners(hwnd: isize) {
    unsafe {
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            HWND(hwnd as _),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

pub fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn error_box(text: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), w!("Pear Lite"), MB_OK | MB_ICONERROR);
    }
}

/// Lower-cased host of an http(s) URL. Like browsers (WHATWG), `\` ends the host as `/` does,
/// so `https://evil.com\@music.youtube.com/` is read as evil.com, the host actually contacted.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let authority = &rest[..rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len())];
    let host = authority.rsplit('@').next()?.split(':').next()?;
    Some(host.to_ascii_lowercase())
}

pub fn is_ytm_url(url: &str) -> bool {
    host_of(url).is_some_and(|h| h == "music.youtube.com")
}

/// Cover art comes from the page and is handed to Windows (media flyout), which fetches it
/// itself, so only https images from Google's CDNs get through. Anything else (`file:`, UNC
/// paths, other hosts) could make Windows connect somewhere it shouldn't.
///
/// The host part must be nothing but letters, digits, `-` and `.`: no `\`, `@`, `:`, `%`,
/// whitespace or control characters. Every URL parser reads such a host the same way, so
/// this check and Windows can't disagree about where the request goes.
pub fn artwork_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host = host.to_ascii_lowercase();
    let plain = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    let is_domain = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    let allowed = plain && (is_domain("ytimg.com") || is_domain("googleusercontent.com") || is_domain("ggpht.com"));
    allowed.then(|| url.to_owned())
}

/// Pages allowed to load inside the app window: YouTube itself plus the Google sign-in flow.
/// Everything else is handed to the default browser.
pub fn is_in_app_url(url: &str) -> bool {
    if url == "about:blank" {
        return true;
    }
    let Some(host) = host_of(url) else { return false };
    let is_domain = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    if is_domain("youtube.com") || is_domain("google.com") {
        return true;
    }
    // Sign-in hops through country domains to set cookies, e.g. accounts.google.co.uk.
    if let Some(tld) = host.strip_prefix("accounts.google.") {
        let parts: Vec<&str> = tld.split('.').collect();
        return parts.len() <= 2 && parts.iter().all(|p| (2..=3).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphabetic()));
    }
    false
}

pub fn autostart_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, APP_NAME, RRF_RT_REG_SZ, None, None, None).is_ok() }
}

pub fn set_autostart(enable: bool) {
    unsafe {
        if enable {
            let Ok(exe) = std::env::current_exe() else { return };
            let cmd = format!("\"{}\" --minimized", exe.display());
            let wide: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                APP_NAME,
                REG_SZ.0,
                Some(wide.as_ptr().cast()),
                (wide.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, APP_NAME);
        }
    }
}

/// Ad / ad-measurement endpoints. Deliberately does NOT include playback stats
/// (`/api/stats/watchtime`, `/api/stats/playback`) — those drive your listening history.
const BLOCKED_PATTERNS: &[&str] = &[
    "*://*.doubleclick.net/*",
    "*://*.googlesyndication.com/*",
    "*://*.googleadservices.com/*",
    "*://*.googletagservices.com/*",
    "*://*.google-analytics.com/*",
    "*://*.2mdn.net/*",
    "*://imasdk.googleapis.com/*",
    "*://*.youtube.com/pagead/*",
    "*://*.youtube.com/api/stats/ads*",
    "*://*.youtube.com/pcs/activeview*",
    "*://*.youtube.com/get_midroll_*",
];

fn is_blocked(uri: &str) -> bool {
    let Some(host) = host_of(uri) else { return false };
    let path = uri.splitn(4, '/').nth(3).unwrap_or("");
    ["doubleclick.net", "googlesyndication.com", "googleadservices.com", "googletagservices.com", "google-analytics.com", "2mdn.net"]
        .iter()
        .any(|d| host.ends_with(d))
        || host == "imasdk.googleapis.com"
        || (host.ends_with("youtube.com")
            && ["pagead/", "api/stats/ads", "pcs/activeview", "get_midroll_"].iter().any(|p| path.starts_with(p)))
}

/// Answers ad requests with an empty 403 before they reach the network.
/// Only URLs matching the filters above ever reach this handler, so normal traffic has no overhead.
pub fn install_request_blocker(webview: &wry::WebView) -> windows::core::Result<()> {
    let core = webview.webview();
    let env = webview.environment();
    unsafe {
        let core22 = core.cast::<ICoreWebView2_22>().ok();
        for pattern in BLOCKED_PATTERNS {
            let filter = HSTRING::from(*pattern);
            match &core22 {
                // Also covers requests from workers and iframes.
                Some(c) => c.AddWebResourceRequestedFilterWithRequestSourceKinds(
                    &filter,
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                )?,
                None => core.AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?,
            }
        }
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            args.Request()?.Uri(&mut uri)?;
            if is_blocked(&take_pwstr(uri)) {
                let response = env.CreateWebResourceResponse(None, 403, w!("Blocked"), w!(""))?;
                args.SetResponse(&response)?;
            }
            Ok(())
        }));
        let mut token = 0i64;
        core.add_WebResourceRequested(&handler, &mut token)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://Music.YouTube.com/watch?v=1").as_deref(), Some("music.youtube.com"));
        assert_eq!(host_of("https://u:p@example.com:8080/x").as_deref(), Some("example.com"));
        assert_eq!(host_of("file:///c:/x"), None);
    }

    #[test]
    fn in_app() {
        assert!(is_in_app_url("https://music.youtube.com/"));
        assert!(is_in_app_url("https://accounts.google.com/ServiceLogin"));
        assert!(is_in_app_url("https://accounts.google.co.uk/accounts/SetSID"));
        assert!(is_in_app_url("https://consent.youtube.com/m"));
        assert!(!is_in_app_url("https://accounts.google.evil.com/"));
        assert!(!is_in_app_url("https://evil.com\\@music.youtube.com/"));
        assert!(!is_ytm_url("https://evil.com\\@music.youtube.com/"));
        assert!(!is_in_app_url("https://notyoutube.com/"));
        assert!(!is_in_app_url("https://open.spotify.com/"));
        assert!(!is_in_app_url("file:///C:/Windows"));
    }

    #[test]
    fn artwork() {
        let ok = |u: &str| artwork_url(u).is_some();
        assert!(ok("https://lh3.googleusercontent.com/abc=w544-h544"));
        assert!(ok("https://i.ytimg.com/vi/x/sddefault.jpg"));
        assert!(ok("https://yt3.ggpht.com/abc"));
        assert!(ok("https://I.YTIMG.com/vi/x.jpg"));
        assert!(!ok("https://i.ytimg.com:443/vi/x.jpg"));
        assert!(!ok("https://i.ytimg.com\t.evil.com/a.jpg"));
        assert!(!ok(" https://i.ytimg.com/a.jpg"));
        assert!(!ok("http://i.ytimg.com/vi/x/sddefault.jpg"));
        assert!(!ok("file://attacker/share/a.jpg"));
        assert!(!ok("\\\\attacker\\share\\a.jpg"));
        assert!(!ok("https://evil.com\\.ytimg.com/a.jpg"));
        assert!(!ok("https://evil.com\\@i.ytimg.com/a.jpg"));
        assert!(!ok("https://i.ytimg.com@evil.com/a.jpg"));
        assert!(!ok("https://user@i.ytimg.com/a.jpg"));
        assert!(!ok("https://i.ytimg.com:8443/a.jpg"));
        assert!(!ok("https://ytimg.com.evil.com/a.jpg"));
        assert!(!ok("https://evil%2ecom/a.jpg"));
        assert!(!ok(""));
    }

    #[test]
    fn blocking() {
        assert!(is_blocked("https://googleads.g.doubleclick.net/pagead/id"));
        assert!(is_blocked("https://music.youtube.com/pagead/viewthroughconversion/1"));
        assert!(is_blocked("https://www.youtube.com/api/stats/ads?ver=2"));
        assert!(!is_blocked("https://music.youtube.com/api/stats/watchtime?x=1"));
        assert!(!is_blocked("https://music.youtube.com/youtubei/v1/player"));
    }
}
