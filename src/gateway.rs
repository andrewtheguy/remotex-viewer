//! One gateway's window: its page, and the shell around it (`shell.js`).

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Fullscreen, Window, WindowBuilder, WindowId};
use wry::{WebContext, WebView, WebViewBuilder};

use crate::UserEvent;
use crate::clipboard::Clipboard;
use crate::os;

const SHELL: &str = include_str!("shell.js");

/// How far each window opens down and right of the one before it, and after how many
/// it starts again from the middle.
const CASCADE_STEP: i32 = 32;
const CASCADE_WRAP: usize = 8;

pub struct Gateway {
    // Dropped in this order: the decoders, the web view, then the window under it.
    #[cfg(windows)]
    media: crate::webview2::Media,
    webview: WebView,
    pub window: Window,
    pub window_focused: bool,
    /// The page's remote surface (its `role="application"` element) has focus.
    pub surface_focused: bool,
}

impl Gateway {
    /// `url`'s page in a window of its own, three quarters of its screen, in the
    /// middle of it and `cascade` steps down and right, as far as the screen allows.
    pub fn open(
        target: &EventLoopWindowTarget<UserEvent>,
        context: &mut WebContext,
        proxy: &EventLoopProxy<UserEvent>,
        url: &str,
        cascade: usize,
        devtools: bool,
    ) -> Result<Self> {
        let window = WindowBuilder::new().with_title("remotex").with_visible(false).build(target)?;
        place(&window, cascade);
        let id = window.id();
        let (ipc, titles) = (proxy.clone(), proxy.clone());
        let builder = WebViewBuilder::new_with_web_context(context)
            .with_url("about:blank")
            .with_initialization_script_for_main_only(SHELL, true)
            .with_ipc_handler(move |request| {
                let _ = ipc.send_event(UserEvent::Page(id, request.into_body()));
            })
            .with_document_title_changed_handler(move |title| {
                let _ = titles.send_event(UserEvent::Title(id, title));
            })
            .with_devtools(devtools);
        #[cfg(windows)]
        let builder = crate::webview2::shim(crate::webview2::configure(builder, devtools));
        let webview = builder.build(&window)?;
        let wanted = proxy.clone();
        let item = os::library_item(
            &window,
            Box::new(move || {
                let _ = wanted.send_event(UserEvent::LibraryWanted);
            }),
        );
        if let Err(e) = item {
            eprintln!("remotex-viewer: {e:#}");
        }
        let gateway = Self {
            #[cfg(windows)]
            media: crate::webview2::Media::new(&webview, crate::webview2::MediaProxy { proxy: proxy.clone(), window: id })?,
            webview,
            window,
            window_focused: false,
            surface_focused: false,
        };
        gateway.webview.load_url(url)?;
        gateway.window.set_visible(true);
        gateway.window.set_focus();
        Ok(gateway)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn page(&mut self, m: &Value, clipboard: &Clipboard) -> Result<()> {
        match m["t"].as_str().unwrap_or("") {
            "shell.fullscreen" => {
                // Borderless on the monitor the window is on: the page's own full screen
                // already hides everything of its own, and exclusive mode would change
                // the display's mode under a desktop that is drawn at its pixels.
                let on = m["on"].as_bool().unwrap_or(false);
                self.window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
            }
            "shell.surface" => self.surface_focused = m["focused"].as_bool().unwrap_or(false),
            "shell.resizeTo" => {
                let dim = |v: &Value| v.as_f64().filter(|v| v.is_finite() && *v >= 1.0).map(|v| v as u32);
                let (w, h) = dim(&m["w"]).zip(dim(&m["h"])).context("resizeTo w, h")?;
                self.resize_to(PhysicalSize::new(w, h));
            }
            "shell.clipboardRead" => clipboard.read(self.id(), m["id"].as_u64().context("id")?),
            "shell.clipboardWrite" => {
                let text = m["text"].as_str().context("text")?.to_owned();
                clipboard.write(self.id(), m["id"].as_u64().context("id")?, text);
            }
            #[cfg(windows)]
            _ => self.media.page(m)?,
            #[cfg(not(windows))]
            other => eprintln!("remotex-viewer: an unknown page message {other:?}"),
        }
        Ok(())
    }

    pub fn clipboard(&self, id: u64, result: Result<String>) -> Result<()> {
        self.send(match result {
            Ok(text) => json!({"t": "clipboard", "id": id, "ok": true, "text": text}),
            Err(e) => json!({"t": "clipboard", "id": id, "ok": false, "error": format!("{e:#}")}),
        })
    }

    pub fn key(&self, code: &str, pressed: bool) -> Result<()> {
        self.send(json!({"t": "key", "code": code, "pressed": pressed}))
    }

    /// Whether the shell holds the keyboard for this page.
    pub fn capture(&self, on: bool) -> Result<()> {
        self.send(json!({"t": "capture", "on": on}))
    }

    #[cfg(windows)]
    pub fn media(&mut self, event: crate::webview2::MediaEvent) -> Result<()> {
        self.media.event(event)
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
        let frame = frame(&self.window);
        let Some((origin, area)) = self.window.current_monitor().map(|m| os::work_area(&m)) else {
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

/// What the window adds around its page, in physical pixels.
pub fn frame(window: &Window) -> PhysicalSize<u32> {
    let (inner, outer) = (window.inner_size(), window.outer_size());
    PhysicalSize::new(outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height))
}

/// Give `window` the outer rectangle `at`, `size`.
pub fn set_outer(window: &Window, at: PhysicalPosition<i32>, size: PhysicalSize<u32>) {
    let frame = frame(window);
    window.set_inner_size(PhysicalSize::new(
        size.width.saturating_sub(frame.width).max(1),
        size.height.saturating_sub(frame.height).max(1),
    ));
    window.set_outer_position(at);
}

/// Three quarters of the screen the window opens on, in the middle of it and `cascade`
/// steps down and right of that: a desktop is often asked to be the window's size, so a
/// small window is a small desktop. A quarter of a narrow screen is fewer than a whole
/// cascade of steps, so the last of them are kept on it.
fn place(window: &Window, cascade: usize) {
    let Some(monitor) = window.current_monitor().or_else(|| window.primary_monitor()) else {
        return;
    };
    let (origin, area) = os::work_area(&monitor);
    let size = PhysicalSize::new(area.width * 3 / 4, area.height * 3 / 4);
    let step = CASCADE_STEP * (cascade % CASCADE_WRAP) as i32;
    let spare = |whole: u32, part: u32| (whole - part) as i32;
    let x = (origin.x + spare(area.width, size.width) / 2 + step).min(origin.x + spare(area.width, size.width));
    let y = (origin.y + spare(area.height, size.height) / 2 + step).min(origin.y + spare(area.height, size.height));
    set_outer(window, PhysicalPosition::new(x, y), size);
}
