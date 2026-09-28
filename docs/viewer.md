# remotex-viewer

`remotex-viewer` (`crates/remotex-viewer`) shows gateways' pages, each in a window of
its own, on Windows now and macOS next. Linux is out of scope: no command at the root
builds the crate, and nothing is done to make it build there.

```sh
remotex-viewer [https://gateway.example/] [--devtools]
```

## The library

The viewer opens on its **library**: the saved gateways as a list, and beside it a form
for the one selected — a name and the gateway's URL, nothing more. It is the library of
the archived `wlshare-windows` client, with the same rules:

- Nothing is saved until you say so. **Save** writes the form into the selected gateway,
  and **Connect** (or Enter, or a double-click on the row) does the same and opens it;
  with nothing selected either makes a new gateway of what is typed. **+** clears the
  form for a new one, which joins the list once it is saved, and **−** deletes the
  selected one.
- Moving to another row, **+**, or closing the library with something unsaved in the
  form asks whether to keep it: **Save**, **Don't Save**, which puts the form back as
  it is saved, or **Cancel**.
- The list is in the order its rows are dragged into, a new gateway joining at the
  bottom. A row is the name and, under it, the URL; with no name, the URL alone.
- A URL is http or https, and a bare host is an https one.

The list, which gateway the form was last showing and where the library window was
left are `profiles.json` in the viewer's data directory (`%LOCALAPPDATA%\remotex-viewer`
on Windows), written beside itself and moved into place. The library opens where it
was last left, clamped into the work area of the screen that place is on, and in the
middle of the screen the first time or when that screen is gone.

### One viewer, many gateways

Each gateway opens in a window of its own, in front of the library, which stays where
it is: connecting adds a window rather than taking the place of anything open, so
several gateways can be open at once. Each window opens at three quarters of its screen,
a step down and right of the one opened before it.

They are all one process. A later launch finds the viewer already running and hands
it what it was launched for — its URL, which opens that gateway, or with none, the
library, brought forward — and ends. Closing the library while a gateway is open only
hides it, and closing the last window, the library or a gateway with the library put
away, ends the viewer.

Gateway windows carry nothing of the library: no button and no bar, only the page. The
way back to it from one is **Library** in the window's system menu — a right click on
the title bar, or Alt+Space while the remote does not hold the keyboard — or launching
the viewer again. In full screen neither is in reach until full screen is left.

On Windows the first launch holds a named mutex and a message-only window, and a later
one sends that window its URL (`os::claim_instance`). On macOS, Launch Services keeps
an app bundle to one copy already.

Every web view shares one data directory (`WebView2` in the data directory), so every
gateway window has the same browser profile, as a browser's tabs do; a gateway's login
is remembered as the browser remembers it.

## The shell

A gateway's window is a **shell**. Everything below the title bar is the SPA the
gateway serves, the same build a browser loads, unchanged and unaware of the shell.
What the shell adds is what the web platform promises that page and a browser window
does not deliver, and it adds it *as* the web platform — the page's full screen, key
events, `navigator.clipboard` and `window.resizeTo` — so there is still one client and
one implementation of every feature in it. `--devtools` turns on WebView2's developer
tools and, for their Inspect item, its context menu, in every window.

It is built from Tauri's own window and web view crates, tao and wry, and arboard, the
clipboard crate Tauri's clipboard plugin uses — not Tauri itself: a library and a
window for each gateway's page need none of Tauri's commands, bundler or
configuration. The same crates run on macOS, where the web view is WKWebView.

### What the shell does

`src/shell.js` is injected into every document of a gateway window that is at the
gateway's origin, on every OS; `src/gateway.rs` answers it, and nothing from any other
origin: a page a link or a redirect leads to is a plain page, with no clipboard of the
shell's and no say over the window. The keyboard is held for one window at a time, the one in
front with its remote surface focused.

| The page does | A browser window | The viewer |
|---|---|---|
| **Immersive full screen** (`requestFullscreen`) | fills the screen | fills the monitor, borderless |
| keys, with the desktop focused | a tab keeps Ctrl+W, Ctrl+T and the like; the OS's own only under Keyboard Lock, in full screen | takes every key from the OS, windowed or not: the Windows key, Alt+Tab, Alt+F4, Ctrl+Esc |
| `navigator.clipboard.readText` on focus | a permission prompt | the system clipboard, no prompt |
| `navigator.clipboard.writeText` for the remote's copy | refused unless focused | written, focused or not |
| **Window → Size to** (`window.resizeTo`) | an installed app window only | sizes this window, kept on its screen |

#### Full screen

The shell follows the page's own `fullscreenchange`: entering element full screen puts
the window in borderless full screen on its monitor, and leaving it restores the
window. A new document reports itself out of full screen, which undoes a reload's
leftover. Borderless rather than exclusive, because exclusive would change the
display's mode under a desktop drawn at its pixels.

#### Keys

While the window is in front and the page's remote surface (its `role="application"`
element) has focus, windowed or full screen, the shell holds the keyboard: an OS hook
(`os::hook_keys`; on Windows a low-level keyboard hook) takes every key ahead of the OS's
shortcuts and the shell hands it to the page, where `shell.js` dispatches it on the
focused element as the `keydown`/`keyup` the page already listens for. The code is the
UI Events code, from tao's scancode tables (`keys::dom_code`); the modifier flags are
the keys the shell is holding; Caps Lock is learnt from the last real event. Keys the
web has no code for stay with the OS, as do injected keys, Ctrl+Alt+Del and Win+L.

A browser hands a page the OS's keys only in full screen, because that is where Keyboard
Lock works; the remote wants them whenever it has the keyboard, so the viewer does not
make the distinction. The way back to local keys is the pointer — a click outside the
window, or on the page's menu, whose controls are not the surface. Anything that takes
focus from the surface, a panel's text box included, hands the keys back at once.
WebView2's browser accelerators are off as well, so while the keys are not held Ctrl+W,
Ctrl+R, F5 and the rest still reach the page, as in an installed app window, and the
window reports `display-mode: standalone` to say it is one.

Holding Escape for 1.5 s leaves full screen, as Chromium's own exit from a locked full
screen does, and the presses reach the remote on the way.

#### Clipboard

`navigator.clipboard.readText` and `writeText` go to the system clipboard through the
shell, on a thread of its own. The page's rules are unchanged: it reads when it gains
focus and writes what the remote copies; the shell only removes the prompt and the
focus requirement a browser puts in front of both.

#### Size to

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
`-p remotex-viewer`, on Windows or macOS. The Windows half needs FFmpeg's prebuilt HEVC decoder, which
`LIBAVCODEC_HEVC_PREBUILT_DIR` points at on `windows-ci-build`.

```powershell
cargo build -p remotex-viewer --release
```

The WebView2 runtime ships with Windows 11 and current Windows 10.

## Not yet

- macOS: the key hook (a `CGEventTap`) and the work area (`NSScreen.visibleFrame`) are
  stubs.
- macOS: a later launch reaches the running viewer as a reopen, which it does not
  take yet, and **Library** belongs in the app's menu bar, which it has no item in yet.
- Packaging, and a remembered size for a gateway's window.
