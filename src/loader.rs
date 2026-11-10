//! Single-file build on the windows-gnu toolchain.
//!
//! There, webview2-com links `WebView2Loader.dll` dynamically (MSVC builds link a static
//! copy instead), which would force shipping the DLL next to the exe. Defining the
//! loader's exports here means the linker never pulls in that import; each call is
//! forwarded to Microsoft's signed loader, embedded in the exe and unpacked on first use.

use std::ffi::c_void;
use std::sync::OnceLock;
use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{HRESULT, HSTRING, PCSTR, PCWSTR, PWSTR};

static LOADER_DLL: &[u8] = include_bytes!("../assets/WebView2Loader.dll");

fn module() -> Option<HMODULE> {
    static MODULE: OnceLock<Option<usize>> = OnceLock::new();
    let handle = MODULE.get_or_init(|| {
        let dir = crate::config::data_dir();
        let path = dir.join("WebView2Loader.dll");
        if std::fs::read(&path).ok().as_deref() != Some(LOADER_DLL) {
            std::fs::create_dir_all(&dir).ok()?;
            let tmp = dir.join("WebView2Loader.dll.tmp");
            std::fs::write(&tmp, LOADER_DLL).ok()?;
            std::fs::rename(&tmp, &path).ok()?;
        }
        unsafe { LoadLibraryW(&HSTRING::from(path.as_path())) }.ok().map(|m| m.0 as usize)
    });
    handle.map(|h| HMODULE(h as *mut c_void))
}

macro_rules! forward {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(no_mangle)]
        pub unsafe extern "system" fn $name($($arg: $ty),*) -> HRESULT {
            type Real = unsafe extern "system" fn($($ty),*) -> HRESULT;
            let name = PCSTR(concat!(stringify!($name), "\0").as_ptr());
            match module().and_then(|m| unsafe { GetProcAddress(m, name) }) {
                Some(f) => unsafe { std::mem::transmute::<_, Real>(f)($($arg),*) },
                None => E_FAIL,
            }
        }
    )*};
}

forward! {
    CreateCoreWebView2EnvironmentWithOptions(folder: PCWSTR, data: PCWSTR, options: *mut c_void, handler: *mut c_void);
    CreateCoreWebView2Environment(handler: *mut c_void);
    GetAvailableCoreWebView2BrowserVersionString(folder: PCWSTR, version: *mut PWSTR);
    GetAvailableCoreWebView2BrowserVersionStringWithOptions(folder: PCWSTR, options: *mut c_void, version: *mut PWSTR);
    CompareBrowserVersions(a: PCWSTR, b: PCWSTR, result: *mut i32);
}
