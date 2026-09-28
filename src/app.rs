use anyhow::{Context, Result};
use serde_json::{Value, json};
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Fullscreen, Window, WindowBuilder};
use wry::{WebView, WebViewBuilder};

use crate::UserEvent;
use crate::clipboard::Clipboard;
use crate::os;

const SHELL: &str = include_str!("shell.js");

struct Args {
    url: String,
    devtools: bool,
}

fn args() -> Result<Args> {
    let mut url = None;
    let mut devtools = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--devtools" => devtools = true,
            flag if flag.starts_with("--") => anyhow::bail!("unknown option {flag}"),
            _ if url.is_none() => url = Some(arg),
            _ => anyhow::bail!("one URL only"),
        }
    }
    Ok(Args { url: url.context("usage: remotex-viewer URL [--devtools]")?, devtools })
}

struct Shell {
    window: Window,
    webview: WebView,
    proxy: EventLoopProxy<UserEvent>,
    clipboard: Clipboard,
    /// The page is in element full screen, so this window fills its monitor.
    fullscreen: bool,
    window_focused: bool,
    /// The page's remote surface (its `role="application"` element) has focus.
    surface_focused: bool,
    keys: Option<os::KeyHook>,
    #[cfg(windows)]
    media: crate::webview2::Media,
}

pub fn run() -> Result<()> {
    let args = args()?;
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("remotex")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .build(&event_loop)?;

    let ipc = proxy.clone();
    let titles = proxy.clone();
    let builder = WebViewBuilder::new()
        .with_url("about:blank")
        .with_initialization_script_for_main_only(SHELL, true)
        .with_ipc_handler(move |request| {
            let _ = ipc.send_event(UserEvent::Page(request.into_body()));
        })
        .with_document_title_changed_handler(move |title| {
            let _ = titles.send_event(UserEvent::Title(title));
        })
        .with_devtools(args.devtools);
    #[cfg(windows)]
    let builder = crate::webview2::configure(builder, args.devtools);
    let webview = builder.build(&window)?;

    let mut shell = Shell {
        #[cfg(windows)]
        media: crate::webview2::Media::new(&webview, proxy.clone())?,
        window,
        clipboard: Clipboard::spawn(proxy.clone()),
        proxy,
        fullscreen: false,
        window_focused: true,
        surface_focused: false,
        keys: None,
        webview,
    };
    shell.webview.load_url(&args.url)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let handled = match event {
            Event::UserEvent(event) => shell.handle(event),
            Event::WindowEvent { event: WindowEvent::Focused(focused), .. } => {
                shell.window_focused = focused;
                shell.update_keys()
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = handled {
            eprintln!("remotex-viewer: {e:#}");
        }
    })
}

impl Shell {
    fn handle(&mut self, event: UserEvent) -> Result<()> {
        match event {
            UserEvent::Page(text) => {
                let message: Value = serde_json::from_str(&text).context("a page message that is not JSON")?;
                self.page(&message)?;
            }
            UserEvent::Title(title) => self.window.set_title(&title),
            UserEvent::Key { code, pressed } => self.send(json!({"t": "key", "code": code, "pressed": pressed}))?,
            UserEvent::Clipboard { id, result } => self.send(match result {
                Ok(text) => json!({"t": "clipboard", "id": id, "ok": true, "text": text}),
                Err(e) => json!({"t": "clipboard", "id": id, "ok": false, "error": format!("{e:#}")}),
            })?,
            #[cfg(windows)]
            other => self.media.event(other)?,
        }
        Ok(())
    }

    fn page(&mut self, m: &Value) -> Result<()> {
        match m["t"].as_str().unwrap_or("") {
            "shell.fullscreen" => {
                self.fullscreen = m["on"].as_bool().unwrap_or(false);
                // Borderless on the monitor the window is on: the page's own full screen
                // already hides everything of its own, and exclusive mode would change
                // the display's mode under a desktop that is drawn at its pixels.
                self.window.set_fullscreen(self.fullscreen.then_some(Fullscreen::Borderless(None)));
                self.update_keys()?;
            }
            "shell.surface" => {
                self.surface_focused = m["focused"].as_bool().unwrap_or(false);
                self.update_keys()?;
            }
            "shell.resizeTo" => {
                let dim = |v: &Value| v.as_f64().filter(|v| v.is_finite() && *v >= 1.0).map(|v| v as u32);
                let (w, h) = dim(&m["w"]).zip(dim(&m["h"])).context("resizeTo w, h")?;
                self.resize_to(PhysicalSize::new(w, h));
            }
            "shell.clipboardRead" => self.clipboard.read(m["id"].as_u64().context("id")?),
            "shell.clipboardWrite" => {
                let text = m["text"].as_str().context("text")?.to_owned();
                self.clipboard.write(m["id"].as_u64().context("id")?, text);
            }
            #[cfg(windows)]
            _ => self.media.page(m)?,
            #[cfg(not(windows))]
            other => eprintln!("remotex-viewer: an unknown page message {other:?}"),
        }
        Ok(())
    }

    /// Hold the keyboard while immersive mode is on screen, in front, with the remote
    /// surface focused — the one arrangement in which a key is plainly meant for the
    /// remote. Windowed, the OS keeps Alt+Tab and the Windows key, as it does for a
    /// windowed Remote Desktop Connection; the browser's own chords reach the page
    /// either way, because the web view reserves none (`webview2::configure`).
    fn update_keys(&mut self) -> Result<()> {
        let want = self.fullscreen && self.window_focused && self.surface_focused;
        if want == self.keys.is_some() {
            return Ok(());
        }
        if want {
            let proxy = self.proxy.clone();
            self.keys = Some(os::hook_keys(
                &self.window,
                Box::new(move |scancode, pressed| match crate::keys::dom_code(scancode) {
                    Some(code) => {
                        let _ = proxy.send_event(UserEvent::Key { code, pressed });
                        true
                    }
                    None => false,
                }),
            )?);
        } else {
            self.keys = None;
        }
        self.send(json!({"t": "capture", "on": want}))
    }

    /// Give the page `inner` physical pixels, on the screen the window is on: as much of
    /// it as fits, with the rest left to scroll, as the page presents any desktop larger
    /// than its window. The frame is measured rather than assumed.
    fn resize_to(&self, inner: PhysicalSize<u32>) {
        if self.window.fullscreen().is_some() {
            return;
        }
        if self.window.is_maximized() {
            self.window.set_maximized(false);
        }
        let (now, outer) = (self.window.inner_size(), self.window.outer_size());
        let frame = PhysicalSize::new(
            outer.width.saturating_sub(now.width),
            outer.height.saturating_sub(now.height),
        );
        let area = os::work_area(&self.window).or_else(|| {
            let monitor = self.window.current_monitor()?;
            Some((monitor.position(), monitor.size()))
        });
        let Some((origin, area)) = area else {
            self.window.set_inner_size(inner);
            return;
        };
        let width = (inner.width + frame.width).min(area.width);
        let height = (inner.height + frame.height).min(area.height);
        let at = self.window.outer_position().unwrap_or(origin);
        let x = at.x.min(origin.x + (area.width - width) as i32).max(origin.x);
        let y = at.y.min(origin.y + (area.height - height) as i32).max(origin.y);
        self.window.set_inner_size(PhysicalSize::new(width - frame.width, height - frame.height));
        self.window.set_outer_position(PhysicalPosition::new(x, y));
    }

    /// Shell -> page. The message is JSON, which is a JavaScript expression.
    fn send(&self, message: Value) -> Result<()> {
        self.webview.evaluate_script(&format!("globalThis.__remotexShell?.receive({message})"))?;
        Ok(())
    }
}
