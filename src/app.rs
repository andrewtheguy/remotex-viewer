//! What holds the viewer together: one library, and a window for every gateway opened
//! from it, in one process.
//!
//! **Connect** adds a gateway window in front of the library, which stays where it is;
//! it never takes one away, so several gateways stand side by side. There is one
//! library, brought forward rather than made again: by a later launch without a URL.
//! Closing the last window — the library, or a gateway with the library put away —
//! ends the viewer.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::WindowId;
use wry::WebContext;

use crate::UserEvent;
use crate::clipboard::Clipboard;
use crate::gateway::Gateway;
use crate::library::Library;
use crate::os;
use crate::profiles::{Store, gateway_url};

struct Args {
    url: Option<String>,
    devtools: bool,
}

fn args() -> Result<Args> {
    let mut url = None;
    let mut devtools = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--devtools" => devtools = true,
            flag if flag.starts_with("--") => anyhow::bail!("unknown option {flag}"),
            _ if url.is_none() => url = Some(gateway_url(&arg)?),
            _ => anyhow::bail!("usage: remotex-viewer [URL] [--devtools]"),
        }
    }
    Ok(Args { url, devtools })
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    /// Every web view's: one data directory, so every gateway window shares one
    /// browser profile, as tabs of a browser do.
    context: WebContext,
    devtools: bool,
    store: Store,
    library: Library,
    gateways: HashMap<WindowId, Gateway>,
    /// How many gateways have been opened, so each window lands a step down and right
    /// of the one before it.
    opened: usize,
    clipboard: Clipboard,
    /// The gateway window holding the keyboard, and the hook that holds it.
    keys: Option<(WindowId, os::KeyHook)>,
    exit: bool,
}

pub fn run() -> Result<()> {
    let args = args()?;
    let Some(instance) = os::claim_instance(args.url.as_deref().unwrap_or(""))? else {
        return Ok(());
    };
    let dir = dirs::data_local_dir().context("no local data directory")?.join("remotex-viewer");
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let launches = proxy.clone();
    instance.listen(Box::new(move |launch| {
        let _ = launches.send_event(UserEvent::Launched(launch));
    }))?;

    let store = Store::open(&dir);
    let mut context = WebContext::new(Some(dir.join("WebView2")));
    let library = Library::new(&event_loop, &mut context, &proxy, args.devtools, store.library())?;
    let mut app = App {
        clipboard: Clipboard::spawn(proxy.clone()),
        proxy,
        context,
        devtools: args.devtools,
        store,
        library,
        gateways: HashMap::new(),
        opened: 0,
        keys: None,
        exit: false,
    };
    // A launch with a URL goes there, and the library, when it is asked for, shows the
    // gateway saved at it; a launch without one asks.
    match &args.url {
        Some(url) => {
            if let Some(profile) = app.store.matching(url) {
                app.library.send(json!({"t": "select", "profile": profile.id}))?;
            }
            app.open(&event_loop, url)?;
        }
        None => app.library.show()?,
    }

    event_loop.run(move |event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let _instance = &instance;
        let handled = match event {
            Event::UserEvent(event) => app.handle(target, event),
            Event::WindowEvent { window_id, event: WindowEvent::Focused(focused), .. } => {
                if let Some(gateway) = app.gateways.get_mut(&window_id) {
                    gateway.window_focused = focused;
                }
                app.update_keys()
            }
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => app.close(window_id),
            _ => Ok(()),
        };
        if let Err(e) = handled {
            eprintln!("remotex-viewer: {e:#}");
        }
        if app.exit {
            *control_flow = ControlFlow::Exit;
        }
    })
}

impl App {
    fn handle(&mut self, target: &EventLoopWindowTarget<UserEvent>, event: UserEvent) -> Result<()> {
        match event {
            UserEvent::Page(id, text) => {
                let message: Value = serde_json::from_str(&text).context("a page message that is not JSON")?;
                if let Some(gateway) = self.gateways.get_mut(&id) {
                    gateway.page(&message, &self.clipboard)?;
                }
                self.update_keys()?;
            }
            UserEvent::Title(id, title) => {
                if let Some(gateway) = self.gateways.get(&id) {
                    gateway.window.set_title(&title);
                }
            }
            UserEvent::Library(text) => {
                let message: Value = serde_json::from_str(&text).context("a library message that is not JSON")?;
                self.library_message(target, &message)?;
            }
            UserEvent::Launched(launch) if launch.is_empty() => self.library.show()?,
            UserEvent::LibraryWanted => self.library.show()?,
            UserEvent::Launched(url) => self.open(target, &url)?,
            UserEvent::Key { code, pressed } => {
                if let Some(gateway) = self.keys.as_ref().and_then(|(id, _)| self.gateways.get(id)) {
                    gateway.key(&code, pressed)?;
                }
            }
            UserEvent::Clipboard { window, id, result } => {
                if let Some(gateway) = self.gateways.get(&window) {
                    gateway.clipboard(id, result)?;
                }
            }
            #[cfg(windows)]
            UserEvent::Media(id, event) => {
                if let Some(gateway) = self.gateways.get_mut(&id) {
                    gateway.media(event)?;
                }
            }
        }
        Ok(())
    }

    /// Open `url` in a window of its own, in front of the library and beside whatever
    /// else is already open.
    fn open(&mut self, target: &EventLoopWindowTarget<UserEvent>, url: &str) -> Result<()> {
        let gateway = Gateway::open(target, &mut self.context, &self.proxy, url, self.opened, self.devtools)?;
        self.opened += 1;
        self.gateways.insert(gateway.id(), gateway);
        Ok(())
    }

    fn close(&mut self, id: WindowId) -> Result<()> {
        if id == self.library.id() {
            // Asked of the form first, which may hold something unsaved; it answers
            // with `leave`, or not at all when the answer is to stay.
            return if self.library.is_ready() { self.library.send(json!({"t": "closing"})) } else { self.leave() };
        }
        self.gateways.remove(&id);
        self.update_keys()?;
        // With nothing else on the screen — no other gateway, and a library that was
        // put away rather than asked for — the viewer goes with it.
        if self.gateways.is_empty() && !self.library.is_visible() {
            self.exit = true;
        }
        Ok(())
    }

    /// The library is closing, its form settled. While a gateway is open it is only
    /// put away, since it is the only library there is; with none, the viewer ends.
    fn leave(&mut self) -> Result<()> {
        if let Some(placement) = self.library.placement() {
            self.store.set_library(placement);
        }
        if self.gateways.is_empty() {
            self.exit = true;
        } else {
            self.library.hide();
        }
        Ok(())
    }

    fn library_message(&mut self, target: &EventLoopWindowTarget<UserEvent>, m: &Value) -> Result<()> {
        let text = |key: &str| m[key].as_str().context("a library message without its text");
        match m["t"].as_str().unwrap_or("") {
            "ready" => {
                let state = self.state();
                self.library.loaded(state)?;
            }
            "put" => {
                let result = self.store.put(m["profile"].as_str(), text("name")?, text("url")?);
                self.reply(m, result.map(|p| json!(p)))?;
            }
            "remove" => {
                let result = self.store.remove(text("profile")?);
                self.reply(m, result.map(|()| Value::Null))?;
            }
            "reorder" => {
                let order: Vec<String> = serde_json::from_value(m["order"].clone()).context("order")?;
                let result = self.store.reorder(&order);
                self.reply(m, result.map(|()| Value::Null))?;
            }
            "select" => self.store.set_selected(m["profile"].as_str().map(str::to_owned)),
            "connect" => {
                let url = self.store.find(text("profile")?).context("connect to a gateway not saved")?.url.clone();
                self.open(target, &url)?;
            }
            "leave" => self.leave()?,
            other => eprintln!("remotex-viewer: an unknown library message {other:?}"),
        }
        Ok(())
    }

    /// The answer to the library's request, with the list as it now is.
    fn reply(&mut self, request: &Value, result: Result<Value>) -> Result<()> {
        let mut reply = match result {
            Ok(value) => json!({"t": "reply", "id": request["id"], "ok": true, "value": value}),
            Err(e) => json!({"t": "reply", "id": request["id"], "ok": false, "error": format!("{e:#}")}),
        };
        reply["state"] = self.state()["state"].take();
        self.library.send(reply)
    }

    fn state(&self) -> Value {
        json!({"t": "state", "state": {"profiles": self.store.profiles(), "selected": self.store.selected()}})
    }

    /// Hold the keyboard for the gateway window in front with its remote surface
    /// focused, windowed or full screen: that is when a key is meant for the remote,
    /// and a browser ties it to full screen only because Keyboard Lock does. The way
    /// out is the pointer, or the page's menu, whose controls are not the surface.
    fn update_keys(&mut self) -> Result<()> {
        let want = self
            .gateways
            .values()
            .find(|g| os::HOLDS_KEYS && g.window_focused && g.surface_focused)
            .map(Gateway::id);
        if want == self.keys.as_ref().map(|(id, _)| *id) {
            return Ok(());
        }
        // The hook goes with the tuple, before another is made.
        if let Some(gateway) = self.keys.take().and_then(|(old, _)| self.gateways.get(&old)) {
            gateway.capture(false)?;
        }
        let Some(gateway) = want.and_then(|id| self.gateways.get(&id)) else {
            return Ok(());
        };
        let proxy = self.proxy.clone();
        let hook = os::hook_keys(
            &gateway.window,
            Box::new(move |scancode, pressed| match crate::keys::dom_code(scancode) {
                Some(code) => {
                    let _ = proxy.send_event(UserEvent::Key { code, pressed });
                    true
                }
                None => false,
            }),
        )?;
        self.keys = Some((gateway.id(), hook));
        gateway.capture(true)
    }
}
