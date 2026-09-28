//! `remotex-viewer URL [--devtools]`: the gateway's page in a window of its own.
//!
//! The window is a shell. Everything below its title bar is the page a browser loads
//! from the same gateway, unchanged; what the shell adds is what the web platform
//! promises that page and a browser window does not deliver (`shell.js`):
//!
//! - immersive mode (the page's element full screen) fills the monitor, and while it
//!   lasts every key goes to the remote, the Windows key and Alt+Tab included;
//! - the page's clipboard is the system clipboard, with no prompt and no focus rule;
//! - `window.resizeTo` sizes this window, so the page's **Size to** item works;
//! - on Windows, the page's HEVC decodes in this process (`webview2`).

#[cfg(any(windows, target_os = "macos"))]
mod app;
#[cfg(any(windows, target_os = "macos"))]
mod clipboard;
#[cfg(any(windows, target_os = "macos"))]
mod keys;
#[cfg(any(windows, target_os = "macos"))]
mod os;
#[cfg(windows)]
mod webview2;

#[cfg(any(windows, target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    app::run()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("remotex-viewer runs on Windows and macOS");
    std::process::exit(2);
}

#[cfg(any(windows, target_os = "macos"))]
pub enum UserEvent {
    /// Page -> shell, as posted.
    Page(String),
    Title(String),
    /// A key taken from the OS while the shell holds the keyboard.
    Key { code: String, pressed: bool },
    /// The answer to the page's clipboard request `id`: the text read, or nothing
    /// for a write.
    Clipboard { id: u64, result: anyhow::Result<String> },
    /// Shell -> page, a JSON message for the HEVC shim.
    #[cfg(windows)]
    Post(String),
    /// A decoder thread needs `count` shared slots of `bytes` each.
    #[cfg(windows)]
    NeedSlots {
        session: u32,
        bytes: usize,
        count: usize,
        reply: std::sync::mpsc::Sender<anyhow::Result<Vec<(u32, usize)>>>,
    },
    /// A slot no decoder will use again.
    #[cfg(windows)]
    Retire(u32),
}
