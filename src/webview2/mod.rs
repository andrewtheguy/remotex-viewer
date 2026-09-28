//! The page's HEVC, decoded here: WebView2 has no HEVC 4:4:4 decoder, and a High
//! Performance Mac's picture is HEVC 4:4:4.
//!
//! `hevc_shim.js` stands in for `VideoDecoder` for `hev1.`/`hvc1.` codecs, in the page
//! and in every worker it starts, and leaves every other codec to WebCodecs. Access
//! units reach this process through a shared ring the page writes; pictures go back in
//! a few read-only shared slots the page builds `VideoFrame`s from. The Mac's AAC-ELD
//! needs nothing here, since WebView2's own `AudioDecoder` takes it.

mod hevc;

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tao::event_loop::EventLoopProxy;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows_core::{HSTRING, Interface};
use wry::{WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

use crate::UserEvent;
use hevc::{Cmd, Session};

const SHIM: &str = include_str!("hevc_shim.js");
const RING_BYTES: usize = 32 << 20;

/// WebView2's part of the window: no browser chords of its own, so Ctrl+W, Ctrl+R,
/// F5 and the rest reach the page as they do in an installed app window; no context
/// menu, since a right click on the remote surface is the remote's (the developer
/// tools keep theirs, for Inspect); and a page that keeps painting, playing and
/// holding its socket while the window is behind another.
pub fn configure(builder: WebViewBuilder<'_>, devtools: bool) -> WebViewBuilder<'_> {
    builder
        .with_initialization_script_for_main_only(SHIM, true)
        .with_browser_accelerator_keys(false)
        .with_default_context_menus(devtools)
        .with_additional_browser_args(
            "--disable-features=CalculateNativeWinOcclusion --disable-renderer-backgrounding \
             --disable-background-timer-throttling --disable-backgrounding-occluded-windows \
             --autoplay-policy=no-user-gesture-required",
        )
}

struct Slot {
    buffer: ICoreWebView2SharedBuffer,
    session: u32,
}

pub struct Media {
    proxy: EventLoopProxy<UserEvent>,
    core: ICoreWebView2,
    core17: ICoreWebView2_17,
    env12: ICoreWebView2Environment12,
    ring: Option<(ICoreWebView2SharedBuffer, usize)>,
    /// Rings of earlier documents: a decoder thread may still read one.
    old_rings: Vec<ICoreWebView2SharedBuffer>,
    sessions: HashMap<u32, Session>,
    slots: HashMap<u32, Slot>,
    next_slot: u32,
}

impl Media {
    pub fn new(webview: &WebView, proxy: EventLoopProxy<UserEvent>) -> Result<Self> {
        let core = webview.webview();
        let env = webview.environment();
        Ok(Self {
            proxy,
            core17: core.cast().context("WebView2 runtime without ICoreWebView2_17 (shared buffers)")?,
            env12: env.cast().context("WebView2 runtime without ICoreWebView2Environment12")?,
            core,
            ring: None,
            old_rings: Vec::new(),
            sessions: HashMap::new(),
            slots: HashMap::new(),
            next_slot: 1,
        })
    }

    pub fn event(&mut self, event: UserEvent) -> Result<()> {
        match event {
            // SAFETY: a COM call on the UI thread.
            UserEvent::Post(json) => unsafe { self.core.PostWebMessageAsJson(&HSTRING::from(json))? },
            UserEvent::NeedSlots { session, bytes, count, reply } => {
                let _ = reply.send(self.make_slots(session, bytes, count));
            }
            UserEvent::Retire(slot) => self.retire(slot)?,
            _ => unreachable!("the shell handles its own events"),
        }
        Ok(())
    }

    pub fn page(&mut self, m: &Value) -> Result<()> {
        let id = m["id"].as_u64().unwrap_or(0) as u32;
        match m["t"].as_str().unwrap_or("") {
            "hello" => self.hello()?,
            "configure" => {
                let session = self.sessions.entry(id).or_insert_with(|| Session::spawn(id, self.proxy.clone()));
                let _ = session.commands.send(Cmd::Configure);
            }
            "decode" => {
                let (_, base) = self.ring.as_ref().context("a decode before the ring")?;
                let off = m["off"].as_u64().context("off")? as usize;
                let len = m["len"].as_u64().context("len")? as usize;
                anyhow::ensure!(off + len <= RING_BYTES, "a unit outside the ring");
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
                let owner = self.slots.get(&slot).map(|s| s.session);
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

    /// A new document: forget the old one's decoders and give it a ring.
    fn hello(&mut self) -> Result<()> {
        for (_, session) in self.sessions.drain() {
            let _ = session.commands.send(Cmd::Close);
        }
        if let Some((ring, _)) = self.ring.take() {
            self.old_rings.push(ring);
        }
        // SAFETY: COM calls on the UI thread; the buffer outlives every pointer taken
        // from it (it is kept in `ring`, then `old_rings`, for the life of the window).
        unsafe {
            let ring = self.env12.CreateSharedBuffer(RING_BYTES as u64)?;
            let mut base = std::ptr::null_mut();
            ring.Buffer(&mut base)?;
            self.core17.PostSharedBufferToScript(
                &ring,
                COREWEBVIEW2_SHARED_BUFFER_ACCESS_READ_WRITE,
                &HSTRING::from(json!({"kind": "in"}).to_string()),
            )?;
            self.ring = Some((ring, base as usize));
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
                    &HSTRING::from(json!({"kind": "slot", "slot": slot, "bytes": bytes}).to_string()),
                )?;
                self.slots.insert(slot, Slot { buffer, session });
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

    fn post(&self, message: Value) -> Result<()> {
        // SAFETY: a COM call on the UI thread.
        unsafe { self.core.PostWebMessageAsJson(&HSTRING::from(message.to_string()))? };
        Ok(())
    }
}
