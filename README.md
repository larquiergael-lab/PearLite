# Pear Lite

A lean Windows port of [pear-desktop](https://github.com/pear-devs/pear-desktop) (the "YouTube Music" desktop app), rewritten in Rust.

Instead of bundling its own Chromium and Node.js the way Electron does, Pear Lite is a single
**~1.3 MB exe** that drives the **Microsoft Edge WebView2 runtime** already built into Windows 10/11.

> Unofficial. Not affiliated with Google, YouTube or the pear-devs team.

## Install

1. Get `PearLite.exe` from this repository's Releases page, or [build it](#build-from-source)
   (it lands in `dist\PearLite.exe`). Put it anywhere, for example `%LOCALAPPDATA%\Programs\PearLite\`.
2. Run it. Optionally pin it to the taskbar, or tick **Start with Windows** in the tray menu.

There's no installer and no other files. Settings live in `%APPDATA%\PearLite`, and the browser
profile (login, cookies, cache) lives in `%LOCALAPPDATA%\PearLite`. Delete those two folders to uninstall completely.

## What it does

| Feature | Notes |
|---|---|
| YouTube Music in a native window | Dark title bar; size, position and last page remembered |
| Stays signed in | Persistent WebView2 profile |
| Mini player | Left-click the tray icon (also inside "Show hidden icons") for a small always-on-top player: artwork, title, artist, a seekable progress bar, and previous / back 10 s / play-pause / forward 10 s / next. Keys: Space, ←/→. Drag it anywhere; it reopens where you left it. It stays until you press its ⌄ button. The ⤢ button or the artwork opens the full window |
| System tray | Right-click menu: now playing, play/pause, next, previous, show/hide window, settings |
| Close to tray | On by default (tray menu toggle) |
| Media keys + Windows media flyout | Shows title, artist and artwork, with working play/pause/next/previous/seek (Windows SMTC) |
| Ad blocking | Ad hosts blocked at the network level, ad payloads stripped from player responses, and a skip/fast-forward fallback |
| Low-memory mode when hidden | Rendering stops and WebView2 trims memory while the window is minimized or in the tray. Audio keeps playing |
| Discord Rich Presence | "Listening to …" with artwork and progress (off by default, tray toggle) |
| Start with Windows | Starts minimized to the tray (tray toggle) |
| Single instance | Launching it again brings the window back |
| Command line | `--play-pause`, `--next`, `--previous`, `--quit` control the running instance (handy for hotkey tools or a Stream Deck). `--minimized` starts in the tray. A `https://music.youtube.com/...` argument opens that page |
| External links | Open in your default browser. Only YouTube and Google sign-in pages load inside the app |
| "Are you still there?" | Answered automatically so music in the tray isn't paused |
| Custom CSS / JS | Drop `custom.css` and/or `custom.js` into `%APPDATA%\PearLite` (applied at startup) |

### Not ported (yet)

pear-desktop has ~40 plugins. These are **not** in Pear Lite: downloader, synced lyrics, equalizer,
visualizer, crossfade, Last.fm scrobbler, API server, SponsorBlock, picture-in-picture, precise volume,
playback speed, taskbar thumbnail buttons, song-change notifications, in-app menu bar and the theme picker
(use `custom.css` instead).

## Resource usage

Measured on this machine (Windows 11, WebView2 155) while playing a music video, muted, summed over every
process the app owns. "Private working set" matches Task Manager's *Memory* column.

| | Private working set | Committed memory | CPU (of one core) |
|---|---|---|---|
| Pear Lite, window visible (default: GPU off) | 170–210 MB | ~255 MB | 4–11% |
| Pear Lite, hidden in tray (default) | 40–120 MB | ~250 MB | ~2% |
| Pear Lite, window visible, GPU on | ~420 MB | ~580 MB | ~10% |
| Pear Lite, hidden in tray, GPU on | 50–95 MB | ~560 MB | 3–4% |
| pear-desktop 3.11 (Electron), for reference | ~437 MB | ~1280 MB | — |
| **Download / disk** | **1.3 MB** exe | | vs ~100 MB installer for Electron |

Ranges are from 2–4 runs each. Notes:
- Most of the memory is the YouTube Music web page itself, in any app that shows it. Pear Lite's own
  process uses about 6 MB.
- GPU acceleration is **off** by default because it halves memory at similar CPU use. Turn it back on
  from the tray menu (**GPU acceleration**, applies on restart) if animations feel choppy on your machine.
- The Electron row is a single snapshot of a separately running instance (paused, not muted), so treat it
  as a ballpark figure, not a controlled comparison.

## Settings

Most settings are in the tray menu. The full list is in `%APPDATA%\PearLite\config.json` (open it with
**Open settings folder** in the tray menu). Edit it while the app is closed:

| Key | Default | Meaning |
|---|---|---|
| `close_to_tray` | `true` | Closing the window hides it to the tray |
| `ad_block` | `true` | Ad blocking (applies on restart) |
| `discord_rpc` | `false` | Discord Rich Presence |
| `discord_client_id` | pear-desktop's app | Use your own Discord application to change the name shown |
| `resume_last_url` | `true` | Reopen the last page (not a `/watch` page, so nothing autoplays at login) |
| `low_memory_when_hidden` | `true` | Trim memory while minimized or hidden |
| `hardware_acceleration` | `false` | GPU rendering and video decoding (applies on restart) |
| `extra_browser_args` | `""` | Extra Chromium switches, appended as-is |
| `devtools` | `false` | Allow F12 developer tools |

Run with `--log` to write a diagnostic log to `%APPDATA%\PearLite\pear-lite.log`.

## Build from source

Requires [Rust](https://rustup.rs) on Windows. Then, from this folder:

```powershell
.\build.ps1          # -> dist\PearLite.exe
.\build.ps1 -Test    # unit tests
.\build.ps1 -Run     # build and start
```

`build.ps1` copies the release exe to `dist\` and embeds the icon, version info and an app manifest
(no `rc.exe` or `windres` needed). It works with either Rust toolchain:

- **MSVC** (needs Visual Studio Build Tools): builds directly.
- **GNU** (`x86_64-pc-windows-gnu`, no Visual Studio needed): rustup's MinGW toolchain can't link the
  modern `windows` crates on its own. Its `dlltool` lacks an assembler, and GNU `ld` mislinks the import
  libraries LLVM writes (the result crashes at startup). The script works around both: it uses LLVM's
  dlltool from rustup's `llvm-tools` component, and it links with `rust-lld` through `tools\ld-shim.rs`,
  which also fixes response-file quoting for paths that contain spaces.

On the GNU toolchain, webview2-com would normally need `WebView2Loader.dll` beside the exe.
`src/loader.rs` embeds Microsoft's signed loader instead and unpacks it to `%LOCALAPPDATA%\PearLite`
on first run, so the app stays a single file.

## Project layout

```
src/main.rs      window, tray, event loop, wiring
src/inject.js    runs inside YouTube Music: ad pruning, media-session tap, commands
src/mini.rs      tray mini player window (UI in src/mini.html)
src/media.rs     Windows media controls (SMTC)
src/discord.rs   Discord Rich Presence (background thread)
src/win.rs       Win32 helpers: single instance, autostart, URL rules, network ad filter
src/loader.rs    single-file WebView2 loader shim (GNU toolchain only)
src/config.rs    settings
build.ps1        build + resource embedding
tools/           build helpers (icon generator, lld shim)
assets/          icons, manifest, WebView2Loader.dll
```

## Credits and licenses

- Pear Lite: MIT ([LICENSE](LICENSE)).
- Icons from pear-desktop, MIT © th-ch ([assets/src/LICENSE-pear-desktop](assets/src/LICENSE-pear-desktop)).
- `assets/WebView2Loader.dll`: © Microsoft, redistributable as part of the WebView2 SDK.
- Built on [wry](https://github.com/tauri-apps/wry), [tao](https://github.com/tauri-apps/tao),
  [tray-icon](https://github.com/tauri-apps/tray-icon), [souvlaki](https://github.com/Sinono3/souvlaki) and
  [discord-rich-presence](https://github.com/vionya/discord-rich-presence).
