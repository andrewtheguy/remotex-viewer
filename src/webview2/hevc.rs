//! One libavcodec HEVC decoder per page `VideoDecoder`, each on its own thread.
//!
//! Access units are read straight out of the page's input ring; each picture is copied,
//! tightly packed, into one of a few shared slots the page maps, and announced as JSON.

use std::collections::VecDeque;
use std::os::raw::c_int;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, mpsc};
use std::time::Instant;

use anyhow::{Context, Result};
use avcodec_hevc_sys::*;
use serde_json::{Value, json};

use super::{MediaEvent, MediaProxy};

/// How many pictures the page may hold between the host's copy and its own.
const SLOTS: usize = 3;

pub enum Cmd {
    Configure,
    Decode { seq: u64, at: usize, len: usize, ts: i64 },
    Flush,
    Reset,
    Close,
}

pub struct Session {
    pub commands: mpsc::Sender<Cmd>,
    pub frees: mpsc::Sender<u32>,
}

impl Session {
    /// A decoder reading `document`'s ring, which it says it no longer does as it ends.
    pub fn spawn(id: u32, document: u64, proxy: MediaProxy, gate: Gate) -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (frees, free_rx) = mpsc::channel();
        let host = Host { proxy, document, gate };
        std::thread::Builder::new()
            .name(format!("hevc-{id}"))
            .spawn(move || {
                run(id, &host, &command_rx, &free_rx);
                let _ = host.proxy.send(MediaEvent::Exited { document });
            })
            .expect("spawn a decoder thread");
        Self { commands, frees }
    }
}

/// Whether a window's shared buffers are still there. A decoder thread reads a ring or
/// writes a slot only while it holds the gate open, and the window closes it before it
/// lets the buffers go: closing waits for whoever holds it, and no one opens it again.
#[derive(Clone)]
pub struct Gate(Arc<RwLock<bool>>);

impl Gate {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(true)))
    }

    /// Held open until the guard is dropped; `None` once the gate is closed.
    fn enter(&self) -> Option<RwLockReadGuard<'_, bool>> {
        let open = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let is_open = *open;
        is_open.then_some(open)
    }

    pub fn close(&self) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = false;
    }
}

/// A decoder thread's way back to its window: for the document it was started for.
struct Host {
    proxy: MediaProxy,
    document: u64,
    gate: Gate,
}

impl Host {
    fn post(&self, message: Value) {
        let _ = self.proxy.send(MediaEvent::Post { document: self.document, message });
    }
}

struct Pool {
    bytes: usize,
    slots: Vec<(u32, usize)>,
    free: VecDeque<u32>,
}

fn run(id: u32, host: &Host, commands: &mpsc::Receiver<Cmd>, frees: &mpsc::Receiver<u32>) {
    let mut decoder: Option<Decoder> = None;
    let mut pool: Option<Pool> = None;
    while let Ok(cmd) = commands.recv() {
        match cmd {
            Cmd::Configure => match Decoder::new() {
                Ok(d) => decoder = Some(d),
                Err(e) => host.post(json!({"t": "error", "id": id, "message": format!("{e:#}")})),
            },
            Cmd::Decode { seq, at, len, ts } => {
                let Some(d) = decoder.as_mut() else {
                    host.post(json!({"t": "done", "id": id, "seq": seq}));
                    continue;
                };
                let started = Instant::now();
                let decoded = {
                    let Some(_open) = host.gate.enter() else { break };
                    // SAFETY: the page wrote `len` bytes at `at` in the ring and does not
                    // reuse them until this unit's `done` is posted below; the ring is
                    // there while the gate is open.
                    let unit = unsafe { std::slice::from_raw_parts(at as *const u8, len) };
                    d.decode(unit, ts)
                };
                match decoded {
                    Ok(got) => {
                        let decode_us = started.elapsed().as_micros() as u64;
                        let mut message = json!({"t": "done", "id": id, "seq": seq, "decodeUs": decode_us});
                        if got {
                            match deliver(id, d, &mut pool, host, frees) {
                                Ok(Some((frame, copy_us))) => {
                                    message["frame"] = frame;
                                    message["copyUs"] = json!(copy_us);
                                }
                                Ok(None) => {
                                    host.post(message);
                                    break;
                                }
                                Err(e) => {
                                    host.post(json!({"t": "error", "id": id, "message": format!("{e:#}")}));
                                    decoder = None;
                                }
                            }
                        }
                        host.post(message);
                    }
                    Err(e) => {
                        host.post(json!({"t": "done", "id": id, "seq": seq}));
                        host.post(json!({"t": "error", "id": id, "message": format!("{e:#}")}));
                        decoder = None;
                    }
                }
            }
            Cmd::Flush => {
                if let Some(d) = decoder.as_mut() {
                    d.drain();
                }
                host.post(json!({"t": "flushed", "id": id}));
            }
            Cmd::Reset => {
                if let Some(d) = decoder.as_mut() {
                    d.reset();
                }
            }
            Cmd::Close => break,
        }
    }
    if let Some(pool) = pool {
        for (slot, _) in pool.slots {
            let _ = host.proxy.send(MediaEvent::Retire(slot));
        }
    }
}

/// Copy the decoder's picture into a free slot. `None` when the page or its window
/// has gone away.
fn deliver(
    id: u32,
    d: &Decoder,
    pool: &mut Option<Pool>,
    host: &Host,
    frees: &mpsc::Receiver<u32>,
) -> Result<Option<(Value, u64)>> {
    // SAFETY: `got` said the frame holds a picture.
    let frame = unsafe { &*d.frame };
    let (format, chroma_w, chroma_h) = match frame.format {
        f if f == AVPixelFormat_AV_PIX_FMT_YUV444P || f == AVPixelFormat_AV_PIX_FMT_YUVJ444P => {
            ("I444", frame.width, frame.height)
        }
        f if f == AVPixelFormat_AV_PIX_FMT_YUV420P || f == AVPixelFormat_AV_PIX_FMT_YUVJ420P => {
            ("I420", (frame.width + 1) / 2, (frame.height + 1) / 2)
        }
        f => anyhow::bail!("libavcodec decoded to pixel format {f}, which the viewer does not pass"),
    };
    let (w, h) = (frame.width as usize, frame.height as usize);
    let (cw, ch) = (chroma_w as usize, chroma_h as usize);
    let bytes = w * h + 2 * cw * ch;

    if pool.as_ref().is_none_or(|p| p.bytes != bytes) {
        if let Some(old) = pool.take() {
            for (slot, _) in old.slots {
                let _ = host.proxy.send(MediaEvent::Retire(slot));
            }
        }
        let (reply, answer) = mpsc::channel();
        host.proxy.send(MediaEvent::NeedSlots { document: host.document, session: id, bytes, count: SLOTS, reply })?;
        let slots = answer.recv().context("the host did not create picture slots")??;
        *pool = Some(Pool { bytes, free: slots.iter().map(|(s, _)| *s).collect(), slots });
    }
    let pool = pool.as_mut().expect("just made");
    let slot = loop {
        if let Some(slot) = pool.free.pop_front() {
            break slot;
        }
        match frees.recv() {
            Ok(slot) if pool.slots.iter().any(|(s, _)| *s == slot) => pool.free.push_back(slot),
            Ok(stale) => {
                let _ = host.proxy.send(MediaEvent::Retire(stale));
            }
            Err(_) => return Ok(None),
        }
    };
    let base = pool.slots.iter().find(|(s, _)| *s == slot).expect("a slot of this pool").1 as *mut u8;

    // Not held while waiting above: the window answers `NeedSlots` on the thread that
    // would close the gate.
    let Some(_open) = host.gate.enter() else {
        return Ok(None);
    };
    let started = Instant::now();
    let planes = [(w, h), (cw, ch), (cw, ch)];
    let mut at = 0usize;
    for (plane, (pw, ph)) in planes.into_iter().enumerate() {
        let stride = frame.linesize[plane] as usize;
        let src = frame.data[plane];
        for row in 0..ph {
            // SAFETY: libavcodec's plane holds `ph` rows of `stride` bytes, at least
            // `pw` of them samples; the slot was made for exactly `bytes`.
            unsafe { std::ptr::copy_nonoverlapping(src.add(row * stride), base.add(at + row * pw), pw) };
        }
        at += pw * ph;
    }
    let copy_us = started.elapsed().as_micros() as u64;

    let full = frame.color_range == AVColorRange_AVCOL_RANGE_JPEG
        || frame.format == AVPixelFormat_AV_PIX_FMT_YUVJ444P
        || frame.format == AVPixelFormat_AV_PIX_FMT_YUVJ420P;
    let space = json!({
        "primaries": primaries(frame.color_primaries),
        "transfer": transfer(frame.color_trc),
        "matrix": matrix(frame.colorspace),
        "fullRange": full,
    });
    Ok(Some((
        json!({"slot": slot, "bytes": bytes, "format": format, "w": w, "h": h, "ts": frame.pts, "space": space}),
        copy_us,
    )))
}

#[allow(non_upper_case_globals)]
fn primaries(p: AVColorPrimaries) -> Value {
    match p {
        AVColorPrimaries_AVCOL_PRI_BT709 => json!("bt709"),
        AVColorPrimaries_AVCOL_PRI_BT470BG => json!("bt470bg"),
        AVColorPrimaries_AVCOL_PRI_SMPTE170M => json!("smpte170m"),
        AVColorPrimaries_AVCOL_PRI_BT2020 => json!("bt2020"),
        AVColorPrimaries_AVCOL_PRI_SMPTE432 => json!("smpte432"),
        _ => Value::Null,
    }
}

#[allow(non_upper_case_globals)]
fn transfer(t: AVColorTransferCharacteristic) -> Value {
    match t {
        AVColorTransferCharacteristic_AVCOL_TRC_BT709 => json!("bt709"),
        AVColorTransferCharacteristic_AVCOL_TRC_SMPTE170M => json!("smpte170m"),
        AVColorTransferCharacteristic_AVCOL_TRC_IEC61966_2_1 => json!("iec61966-2-1"),
        AVColorTransferCharacteristic_AVCOL_TRC_LINEAR => json!("linear"),
        AVColorTransferCharacteristic_AVCOL_TRC_SMPTE2084 => json!("pq"),
        AVColorTransferCharacteristic_AVCOL_TRC_ARIB_STD_B67 => json!("hlg"),
        _ => Value::Null,
    }
}

#[allow(non_upper_case_globals)]
fn matrix(m: AVColorSpace) -> Value {
    match m {
        AVColorSpace_AVCOL_SPC_RGB => json!("rgb"),
        AVColorSpace_AVCOL_SPC_BT709 => json!("bt709"),
        AVColorSpace_AVCOL_SPC_BT470BG => json!("bt470bg"),
        AVColorSpace_AVCOL_SPC_SMPTE170M => json!("smpte170m"),
        AVColorSpace_AVCOL_SPC_BT2020_NCL => json!("bt2020-ncl"),
        _ => Value::Null,
    }
}

struct Decoder {
    ctx: *mut AVCodecContext,
    packet: *mut AVPacket,
    frame: *mut AVFrame,
}

impl Decoder {
    fn new() -> Result<Self> {
        // SAFETY: every allocation is checked before use; Drop frees each, null or not.
        unsafe {
            av_log_set_level(AV_LOG_QUIET);
            let codec = avcodec_find_decoder(AVCodecID_AV_CODEC_ID_HEVC);
            anyhow::ensure!(!codec.is_null(), "libavcodec has no HEVC decoder");
            let d = Self { ctx: avcodec_alloc_context3(codec), packet: av_packet_alloc(), frame: av_frame_alloc() };
            anyhow::ensure!(
                !d.ctx.is_null() && !d.packet.is_null() && !d.frame.is_null(),
                "libavcodec could not allocate a decoder"
            );
            // The Mac codes with WPP, so rows decode on slice threads; frame threads
            // would add a picture of delay and a picture of memory per thread.
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
            (*d.ctx).thread_count = threads as c_int;
            (*d.ctx).thread_type = FF_THREAD_SLICE as c_int;
            (*d.ctx).flags |= AV_CODEC_FLAG_LOW_DELAY as c_int;
            (*d.ctx).err_recognition |= AV_EF_EXPLODE as c_int;
            let err = avcodec_open2(d.ctx, codec, std::ptr::null_mut());
            anyhow::ensure!(err >= 0, "libavcodec could not open the HEVC decoder: {}", text(err));
            Ok(d)
        }
    }

    /// Decode one Annex B access unit. True when `frame` now holds a picture.
    fn decode(&mut self, unit: &[u8], ts: i64) -> Result<bool> {
        // SAFETY: a live context, packet and frame. The packet owns no buffer, so
        // `avcodec_send_packet` copies the unit, padded, before it returns.
        unsafe {
            av_frame_unref(self.frame);
            (*self.packet).data = unit.as_ptr().cast_mut();
            (*self.packet).size = c_int::try_from(unit.len()).context("an access unit too large")?;
            (*self.packet).pts = ts;
            let err = avcodec_send_packet(self.ctx, self.packet);
            (*self.packet).data = std::ptr::null_mut();
            (*self.packet).size = 0;
            anyhow::ensure!(err >= 0, "libavcodec refused an access unit: {}", text(err));
            // One picture per unit, out at once: the stream reorders nothing.
            let err = avcodec_receive_frame(self.ctx, self.frame);
            if err == AVERROR_EAGAIN {
                return Ok(false);
            }
            anyhow::ensure!(err >= 0, "libavcodec: {}", text(err));
            Ok(true)
        }
    }

    /// WebCodecs `flush`: finish everything sent. Nothing is reordered in this stream,
    /// so there is nothing left to output; the decoder is readied for the next unit.
    fn drain(&mut self) {
        // SAFETY: a live context and frame.
        unsafe {
            avcodec_send_packet(self.ctx, std::ptr::null());
            while avcodec_receive_frame(self.ctx, self.frame) >= 0 {
                av_frame_unref(self.frame);
            }
            avcodec_flush_buffers(self.ctx);
        }
    }

    fn reset(&mut self) {
        // SAFETY: a live context.
        unsafe { avcodec_flush_buffers(self.ctx) };
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: freed exactly once; each call takes null and nulls its pointer.
        unsafe {
            avcodec_free_context(&mut self.ctx);
            av_packet_free(&mut self.packet);
            av_frame_free(&mut self.frame);
        }
    }
}

fn text(err: c_int) -> String {
    let mut text = [0 as std::os::raw::c_char; AV_ERROR_MAX_STRING_SIZE as usize];
    // SAFETY: av_strerror writes a NUL-terminated string within the given length.
    unsafe {
        av_strerror(err, text.as_mut_ptr(), text.len());
        std::ffi::CStr::from_ptr(text.as_ptr()).to_string_lossy().into_owned()
    }
}
