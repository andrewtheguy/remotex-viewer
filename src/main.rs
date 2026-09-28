//! `remotex-viewer [URL] [--devtools]`: gateways' pages, each in a window of its own.
//!
//! The viewer opens on its library: the saved gateways, a name and a URL each, and a
//! form for the one selected. **Connect** opens that gateway in a window of its own and
//! leaves the library where it is, so several gateways can be open at once, all in one
//! process: a later launch hands its URL, or its wish for the library, to the viewer
//! already running. A URL on the command line opens that gateway without the library.
//!
//! A gateway's window is a shell. Everything below its title bar is the page a browser
//! loads from the same gateway, unchanged; what the shell adds is what the web platform
//! promises that page and a browser window does not deliver (`shell.js`):
//!
//! - immersive mode (the page's element full screen) fills the monitor;
//! - while the remote surface has focus, every key goes to the remote, the Windows key
//!   and Alt+Tab included, windowed or not;
//! - the page's clipboard is the system clipboard, with no prompt and no focus rule;
//! - `window.resizeTo` sizes this window, so the page's **Size to** item works;
//! - on Windows, the page's HEVC decodes in this process (`webview2`).

mod app;
mod clipboard;
mod gateway;
mod keys;
mod library;
mod os;
mod profiles;
#[cfg(windows)]
mod webview2;

fn main() -> anyhow::Result<()> {
    app::run()
}

pub enum UserEvent {
    /// A gateway's page -> its shell, as posted.
    Page(tao::window::WindowId, String),
    Title(tao::window::WindowId, String),
    /// The library's page -> the viewer, as posted.
    Library(String),
    /// A later launch, handed over: its gateway URL, or empty for the library.
    Launched(String),
    /// A key taken from the OS for the gateway window holding the keyboard.
    Key { code: String, pressed: bool },
    /// The answer to a gateway page's clipboard request `id`: the text read, or
    /// nothing for a write.
    Clipboard { window: tao::window::WindowId, id: u64, result: anyhow::Result<String> },
    /// A decoder thread's request of its window's web view.
    #[cfg(windows)]
    Media(tao::window::WindowId, webview2::MediaEvent),
}
