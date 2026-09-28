//! The library: the one window the saved gateways and their form live in
//! (`library.html`). Made once and brought forward or put away rather than made again,
//! and every gateway window is opened from it; it stays where it is when one is, behind
//! the gateway that opens in front. It opens where it was last left.

use anyhow::Result;
use serde_json::{Value, json};
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{WebContext, WebView, WebViewBuilder};

use crate::UserEvent;
use crate::gateway::set_outer;
use crate::os;
use crate::profiles::Placement;

const PAGE: &str = include_str!("library.html");

pub struct Library {
    webview: WebView,
    window: Window,
    /// The page has loaded and takes messages; until then they wait.
    ready: bool,
    pending: Vec<Value>,
    /// It has been on the screen, so where it is is worth keeping.
    shown: bool,
}

impl Library {
    /// Made hidden, at `left` if that is still on a screen.
    pub fn new(
        target: &EventLoopWindowTarget<UserEvent>,
        context: &mut WebContext,
        proxy: &EventLoopProxy<UserEvent>,
        devtools: bool,
        left: Option<Placement>,
    ) -> Result<Self> {
        let window = WindowBuilder::new().with_title("remotex").with_visible(false).build(target)?;
        place(&window, left);
        let ipc = proxy.clone();
        let builder = WebViewBuilder::new_with_web_context(context)
            .with_html(PAGE)
            .with_ipc_handler(move |request| {
                let _ = ipc.send_event(UserEvent::Library(request.into_body()));
            })
            .with_devtools(devtools);
        #[cfg(windows)]
        let builder = crate::webview2::configure(builder, devtools);
        let webview = builder.build(&window)?;
        Ok(Self { webview, window, ready: false, pending: Vec::new(), shown: false })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    /// Bring it forward, and have the form put the cursor where typing starts.
    pub fn show(&mut self) -> Result<()> {
        self.window.set_visible(true);
        if self.window.is_minimized() {
            self.window.set_minimized(false);
        }
        self.window.set_focus();
        self.shown = true;
        self.send(json!({"t": "show"}))
    }

    /// Out of the way behind the gateway windows: not closed, since it is the only
    /// library there is.
    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    /// Where it is, to open there next time — once it has been on the screen at all,
    /// and as a plain window, not the frame a maximized one fills, which would come
    /// back as an ordinary window the size of the screen.
    pub fn placement(&self) -> Option<Placement> {
        let w = &self.window;
        if !self.shown || !w.is_visible() || w.is_maximized() || w.is_minimized() {
            return None;
        }
        let at = w.outer_position().ok()?;
        let size = w.outer_size();
        Some(Placement { x: at.x, y: at.y, width: size.width, height: size.height })
    }

    /// The page has loaded: `first` goes to it, then what waited.
    pub fn loaded(&mut self, first: Value) -> Result<()> {
        self.ready = true;
        self.eval(&first)?;
        for message in std::mem::take(&mut self.pending) {
            self.eval(&message)?;
        }
        Ok(())
    }

    pub fn send(&mut self, message: Value) -> Result<()> {
        if !self.ready {
            self.pending.push(message);
            return Ok(());
        }
        self.eval(&message)
    }

    fn eval(&self, message: &Value) -> Result<()> {
        self.webview.evaluate_script(&format!("globalThis.library?.receive({message})"))?;
        Ok(())
    }
}

/// Where it was last left, within the work area of the screen that place is most on,
/// as far as it fits: a window left on a screen that has since shrunk is not left
/// hanging off it. The first time, and when that place is on no screen any more, three
/// quarters of the screen it opens on, in the middle of it.
fn place(window: &Window, left: Option<Placement>) {
    let overlap = |p: &Placement, (at, size): (PhysicalPosition<i32>, PhysicalSize<u32>)| {
        let span = |a: i32, a_len: u32, b: i32, b_len: u32| {
            (i64::from(a) + i64::from(a_len)).min(i64::from(b) + i64::from(b_len)) - i64::from(a.max(b))
        };
        let (w, h) = (span(p.x, p.width, at.x, size.width), span(p.y, p.height, at.y, size.height));
        if w > 0 && h > 0 { w * h } else { 0 }
    };
    if let Some(left) = left {
        let screen = window
            .available_monitors()
            .map(|m| (overlap(&left, (m.position(), m.size())), m))
            .filter(|(o, _)| *o > 0)
            .max_by_key(|(o, _)| *o);
        if let Some((_, monitor)) = screen {
            let (origin, area) = os::work_area(&monitor);
            let size = PhysicalSize::new(left.width.min(area.width), left.height.min(area.height));
            let x = left.x.clamp(origin.x, origin.x + (area.width - size.width) as i32);
            let y = left.y.clamp(origin.y, origin.y + (area.height - size.height) as i32);
            set_outer(window, PhysicalPosition::new(x, y), size);
            return;
        }
    }
    let Some(monitor) = window.current_monitor().or_else(|| window.primary_monitor()) else {
        return;
    };
    let (origin, area) = os::work_area(&monitor);
    let size = PhysicalSize::new(area.width * 3 / 4, area.height * 3 / 4);
    let at = PhysicalPosition::new(
        origin.x + (area.width - size.width) as i32 / 2,
        origin.y + (area.height - size.height) as i32 / 2,
    );
    set_outer(window, at, size);
}
