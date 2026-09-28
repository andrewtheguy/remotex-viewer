//! The page's HEVC, decoded here: WebView2 has no HEVC 4:4:4 decoder, and a High
//! Performance Mac's picture is HEVC 4:4:4.
//!
//! `hevc_shim.js` stands in for `VideoDecoder` for `hev1.`/`hvc1.` codecs, in the page
//! and in every worker it starts, and leaves every other codec to WebCodecs. Access
//! units reach this process through a shared ring the page writes; pictures go back in
//! a few read-only shared slots the page builds `VideoFrame`s from. The Mac's AAC-ELD
//! needs nothing here, since WebView2's own `AudioDecoder` takes it.
//!
//! Session and sequence numbers start again in every document, so everything the host
//! sends names the document it is for, by the token the shim chose in its `hello`, and
//! everything a decoder thread asks is dropped once its document has gone.

mod hevc;

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tao::event_loop::EventLoopProxy;
use tao::window::WindowId;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows_core::{HSTRING, Interface};
use wry::{WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

use crate::UserEvent;
use hevc::{Cmd, Gate, Session};

pub const SHIM: &str = include_str!("hevc_shim.js");
const RING_BYTES: usize = 32 << 20;

/// WebView2's part of every web view, the library's included: no browser chords of its
/// own, so Ctrl+W, Ctrl+R, F5 and the rest reach the page as they do in an installed
/// app window; no context menu, since a right click on the remote surface is the
/// remote's (the developer tools keep theirs, for Inspect); and a page that keeps
/// painting, playing and holding its socket while the window is behind another. The
/// web views share one data directory, and WebView2 refuses a second set of browser
/// arguments over it, so every one is given the same.
pub fn configure(builder: WebViewBuilder<'_>, devtools: bool) -> WebViewBuilder<'_> {
    builder
        .with_browser_accelerator_keys(false)
        .with_default_context_menus(devtools)
        .with_additional_browser_args(
            "--disable-features=CalculateNativeWinOcclusion --disable-renderer-backgrounding \
             --disable-background-timer-throttling --disable-backgrounding-occluded-windows \
             --autoplay-policy=no-user-gesture-required",
        )
}

/// What a decoder thread asks of its window's web view, on the UI thread. `document`
/// is the one the thread was started for.
pub enum MediaEvent {
    /// A message for the shim.
    Post { document: u64, message: Value },
    /// A decoder needs `count` shared slots of `bytes` each.
    NeedSlots {
        document: u64,
        session: u32,
        bytes: usize,
        count: usize,
        reply: std::sync::mpsc::Sender<Result<Vec<(u32, usize)>>>,
    },
    /// A slot no decoder will use again.
    Retire(u32),
    /// A decoder thread has ended, and reads its document's ring no more.
    Exited { document: u64 },
}

/// The event loop, addressed to one gateway window's `Media`.
#[derive(Clone)]
pub struct MediaProxy {
    pub proxy: EventLoopProxy<UserEvent>,
    pub window: WindowId,
}

impl MediaProxy {
    /// Fails once the event loop has gone.
    pub fn send(&self, event: MediaEvent) -> Result<()> {
        self.proxy
            .send_event(UserEvent::Media(self.window, event))
            .map_err(|_| anyhow::anyhow!("the event loop has gone"))
    }
}

/// The shared buffer one document writes its access units into.
struct Ring {
    buffer: ICoreWebView2SharedBuffer,
    base: usize,
    /// Decoder threads started for the document and not yet ended: any may still read it.
    decoders: usize,
}

struct Slot {
    buffer: ICoreWebView2SharedBuffer,
    document: u64,
    session: u32,
}

pub struct Media {
    proxy: MediaProxy,
    core: ICoreWebView2,
    core17: ICoreWebView2_17,
    env12: ICoreWebView2Environment12,
    /// The current document, counted from the first, and the token its shim chose.
    document: u64,
    token: String,
    /// Its ring, and each earlier document's while a decoder thread may still read it.
    rings: HashMap<u64, Ring>,
    sessions: HashMap<u32, Session>,
    slots: HashMap<u32, Slot>,
    next_slot: u32,
    /// Held open by a decoder thread while it reads a ring or writes a slot.
    gate: Gate,
}

impl Media {
    pub fn new(webview: &WebView, proxy: MediaProxy) -> Result<Self> {
        let core = webview.webview();
        let env = webview.environment();
        Ok(Self {
            proxy,
            core17: core.cast().context("WebView2 runtime without ICoreWebView2_17 (shared buffers)")?,
            env12: env.cast().context("WebView2 runtime without ICoreWebView2Environment12")?,
            core,
            document: 0,
            token: String::new(),
            rings: HashMap::new(),
            sessions: HashMap::new(),
            slots: HashMap::new(),
            next_slot: 1,
            gate: Gate::new(),
        })
    }

    pub fn event(&mut self, event: MediaEvent) -> Result<()> {
        match event {
            MediaEvent::Post { document, message } => {
                if document == self.document {
                    self.post(message)?;
                }
            }
            MediaEvent::NeedSlots { document, session, bytes, count, reply } => {
                let slots = if document == self.document {
                    self.make_slots(session, bytes, count)
                } else {
                    Err(anyhow::anyhow!("the decoder's document has gone"))
                };
                let _ = reply.send(slots);
            }
            MediaEvent::Retire(slot) => self.retire(slot)?,
            MediaEvent::Exited { document } => {
                if let Some(ring) = self.rings.get_mut(&document) {
                    ring.decoders -= 1;
                }
                self.close_rings()?;
            }
        }
        Ok(())
    }

    pub fn page(&mut self, m: &Value) -> Result<()> {
        let id = m["id"].as_u64().unwrap_or(0) as u32;
        match m["t"].as_str().unwrap_or("") {
            "hello" => self.hello(m["doc"].as_str().context("a hello without its document")?)?,
            "configure" => {
                let session = self.sessions.entry(id).or_insert_with(|| {
                    if let Some(ring) = self.rings.get_mut(&self.document) {
                        ring.decoders += 1;
                    }
                    Session::spawn(id, self.document, self.proxy.clone(), self.gate.clone())
                });
                let _ = session.commands.send(Cmd::Configure);
            }
            "decode" => {
                let base = self.rings.get(&self.document).context("a decode before the ring")?.base;
                let off = usize::try_from(m["off"].as_u64().context("off")?).context("off")?;
                let len = usize::try_from(m["len"].as_u64().context("len")?).context("len")?;
                let end = off.checked_add(len);
                anyhow::ensure!(end.is_some_and(|end| end <= RING_BYTES), "a unit outside the ring");
                let cmd = Cmd::Decode {
                    seq: m["seq"].as_u64().context("seq")?,
                    at: base + off,
                    len,
                    ts: m["ts"].as_f64().unwrap_or(0.0) as i64,
                };
                match self.sessions.get(&id) {
                    Some(session) => {
                        let _ = session.commands.send(cmd);
                    }
                    None => self.post(json!({"t": "done", "id": id, "seq": m["seq"]}))?,
                }
            }
            "flush" => match self.sessions.get(&id) {
                Some(session) => {
                    let _ = session.commands.send(Cmd::Flush);
                }
                None => self.post(json!({"t": "flushed", "id": id}))?,
            },
            "reset" => {
                if let Some(session) = self.sessions.get(&id) {
                    let _ = session.commands.send(Cmd::Reset);
                }
            }
            "close" => {
                if let Some(session) = self.sessions.remove(&id) {
                    let _ = session.commands.send(Cmd::Close);
                }
            }
            "free" => {
                let slot = m["slot"].as_u64().context("slot")? as u32;
                let owner = self.slots.get(&slot).filter(|s| s.document == self.document).map(|s| s.session);
                match owner.and_then(|session| self.sessions.get(&session)) {
                    Some(session) => {
                        let _ = session.frees.send(slot);
                    }
                    None => self.retire(slot)?,
                }
            }
            "log" => println!("page: {}", m["text"].as_str().unwrap_or("")),
            other => eprintln!("remotex-viewer: an unknown page message {other:?}"),
        }
        Ok(())
    }

    /// A new document, `token` in its messages: forget the old one's decoders and give
    /// it a ring. The old ring is kept until the last of them has ended.
    fn hello(&mut self, token: &str) -> Result<()> {
        for (_, session) in self.sessions.drain() {
            let _ = session.commands.send(Cmd::Close);
        }
        self.document += 1;
        token.clone_into(&mut self.token);
        self.close_rings()?;
        // SAFETY: COM calls on the UI thread; the buffer outlives every pointer taken
        // from it (it is kept in `rings` until no decoder thread reads it).
        unsafe {
            let ring = self.env12.CreateSharedBuffer(RING_BYTES as u64)?;
            let mut base = std::ptr::null_mut();
            ring.Buffer(&mut base)?;
            self.core17.PostSharedBufferToScript(
                &ring,
                COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE,
                &HSTRING::from(json!({"kind": "in", "doc": self.token}).to_string()),
            )?;
            self.rings.insert(self.document, Ring { buffer: ring, base: base as usize, decoders: 0 });
        }
        Ok(())
    }

    /// Close each earlier document's ring that no decoder thread reads any more.
    fn close_rings(&mut self) -> Result<()> {
        let unread: Vec<u64> =
            self.rings.iter().filter(|(d, r)| **d != self.document && r.decoders == 0).map(|(d, _)| *d).collect();
        for document in unread {
            let ring = self.rings.remove(&document).expect("just found");
            // SAFETY: a COM call on the UI thread.
            unsafe { ring.buffer.Close()? };
        }
        Ok(())
    }

    fn make_slots(&mut self, session: u32, bytes: usize, count: usize) -> Result<Vec<(u32, usize)>> {
        let mut made = Vec::with_capacity(count);
        for _ in 0..count {
            let slot = self.next_slot;
            self.next_slot += 1;
            // SAFETY: COM calls on the UI thread; the buffer is kept in `slots` until
            // retired, which only happens once its decoder no longer writes it.
            unsafe {
                let buffer = self.env12.CreateSharedBuffer(bytes as u64)?;
                let mut base = std::ptr::null_mut();
                buffer.Buffer(&mut base)?;
                self.core17.PostSharedBufferToScript(
                    &buffer,
                    COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_ONLY,
                    &HSTRING::from(
                        json!({"kind": "slot", "doc": self.token, "slot": slot, "bytes": bytes}).to_string(),
                    ),
                )?;
                self.slots.insert(slot, Slot { buffer, document: self.document, session });
                made.push((slot, base as usize));
            }
        }
        Ok(made)
    }

    fn retire(&mut self, slot: u32) -> Result<()> {
        if let Some(entry) = self.slots.remove(&slot) {
            self.post(json!({"t": "retire", "slot": slot}))?;
            // SAFETY: a COM call on the UI thread.
            unsafe { entry.buffer.Close()? };
        }
        Ok(())
    }

    /// `message`, for the current document.
    fn post(&self, mut message: Value) -> Result<()> {
        message["doc"] = json!(self.token);
        // SAFETY: a COM call on the UI thread.
        unsafe { self.core.PostWebMessageAsJson(&HSTRING::from(message.to_string()))? };
        Ok(())
    }
}

impl Drop for Media {
    /// Waits out any decoder thread reading a ring or writing a slot, and keeps every
    /// one from starting again, before the buffers go.
    fn drop(&mut self) {
        self.gate.close();
    }
}
