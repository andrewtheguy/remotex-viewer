# remotex-viewer

`remotex-viewer` (`crates/remotex-viewer`) shows a gateway's page in a window of its
own, on Windows now and macOS next; Linux is out of scope, and there the binary only
says so.

```sh
remotex-viewer https://gateway.example/ [--devtools]
```

It is a **shell**. Everything below the title bar is the SPA the gateway serves, the
same build a browser loads, unchanged and unaware of the shell. What the shell adds is
what the web platform promises that page and a browser window does not deliver, and it
adds it *as* the web platform — the page's full screen, key events, `navigator.clipboard`
and `window.resizeTo` — so there is still one client and one implementation of every
feature in it. `--devtools` turns on WebView2's developer tools and, for their Inspect
item, its context menu.

It is built from Tauri's own window and web view crates, tao and wry, and arboard, the
clipboard crate Tauri's clipboard plugin uses — not Tauri itself: there is one window
showing a remote page, and nothing here needs Tauri's commands, bundler or
configuration. The same crates run on macOS, where the web view is WKWebView.

## What the shell does

`src/shell.js` is injected into every document, on every OS; `src/app.rs` answers it.

| The page does | A browser window | The viewer |
|---|---|---|
| **Immersive full screen** (`requestFullscreen`) | fills the screen | fills the monitor, borderless |
| keys, in immersive mode with the desktop focused | Keyboard Lock, when the browser grants it | takes every key from the OS: the Windows key, Alt+Tab, Alt+F4, Ctrl+Esc |
| keys, windowed | a tab keeps Ctrl+W, Ctrl+T and the like | every browser chord reaches the page; the OS keeps its own |
| `navigator.clipboard.readText` on focus | a permission prompt | the system clipboard, no prompt |
| `navigator.clipboard.writeText` for the remote's copy | refused unless focused | written, focused or not |
| **Window → Size to** (`window.resizeTo`) | an installed app window only | sizes this window, kept on its screen |

### Full screen

The shell follows the page's own `fullscreenchange`: entering element full screen puts
the window in borderless full screen on its monitor, and leaving it restores the
window. A new document reports itself out of full screen, which undoes a reload's
leftover. Borderless rather than exclusive, because exclusive would change the
display's mode under a desktop drawn at its pixels.

### Keys

While the page is in full screen, the window is in front and the page's remote surface
(its `role="application"` element) has focus, the shell holds the keyboard: an OS hook
(`os::hook_keys`; on Windows a low-level keyboard hook) takes every key ahead of the OS's
shortcuts and the shell hands it to the page, where `shell.js` dispatches it on the
focused element as the `keydown`/`keyup` the page already listens for. The code is the
UI Events code, from tao's scancode tables (`keys::dom_code`); the modifier flags are
the keys the shell is holding; Caps Lock is learnt from the last real event. Keys the
web has no code for stay with the OS, as do injected keys, Ctrl+Alt+Del and Win+L.

Windowed, nothing is hooked, as with a windowed Remote Desktop Connection: Alt+Tab and
the Windows key stay the OS's. WebView2's browser accelerators are off, so Ctrl+W,
Ctrl+R, F5 and the rest reach the page windowed too, as in an installed app window, and
the window reports `display-mode: standalone` to say it is one.

Holding Escape for 1.5 s leaves full screen, as Chromium's own exit from a locked full
screen does, and the presses reach the remote on the way.

### Clipboard

`navigator.clipboard.readText` and `writeText` go to the system clipboard through the
shell, on a thread of its own. The page's rules are unchanged: it reads when it gains
focus and writes what the remote copies; the shell only removes the prompt and the
focus requirement a browser puts in front of both.

### Size to

`window.resizeTo` asks for an outer size the page computed from the inner one it wants.
`shell.js` recovers the inner size in physical pixels; the shell adds its measured frame,
fits the result to the monitor's work area and keeps the window on it. A desktop larger
than the screen scrolls, as it does in any window.

## HEVC on Windows

No Windows browser decodes a High Performance Mac's HEVC 4:4:4, so WebView2 gets it from
the viewer (`src/webview2/`). `hevc_shim.js` stands in for `VideoDecoder` for `hev1.` and
`hvc1.` codecs, in the page and in every worker the page starts, and leaves every other
codec — VP9 included — to WebCodecs. Access units reach the viewer through a 32 MB shared
ring the page writes; FFmpeg's HEVC decoder (the one `apple-hp-media` links) decodes each
on a thread per decoder; the picture goes back through one of three read-only shared
slots, from which the page builds an `I444` `VideoFrame`. With the shim, the page's load
time question answers yes to the Mac's HEVC, so a target with `media_passthrough` passes
it. The Mac's AAC-ELD needs nothing: WebView2's own `AudioDecoder` takes it in the raw
AudioSpecificConfig form the page already probes for.

On macOS, WKWebView decodes the Mac's HEVC itself, so none of this applies there.

## Building

It is a workspace member that commands at the root never build: pick it with
`-p remotex-viewer`. The Windows half needs FFmpeg's prebuilt HEVC decoder, which
`LIBAVCODEC_HEVC_PREBUILT_DIR` points at on `windows-ci-build`.

```powershell
cargo build -p remotex-viewer --release
```

The WebView2 runtime ships with Windows 11 and current Windows 10.

## Not yet

- macOS: the key hook (a `CGEventTap`) and the work area (`NSScreen.visibleFrame`) are
  stubs.
- Packaging, a remembered gateway URL and window size, and a data directory of its own
  (WebView2 keeps its profile beside the executable).
