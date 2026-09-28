// Injected into every document (AddScriptToExecuteOnDocumentCreated, main frame only).
//
// Puts the host's HEVC decoder behind `VideoDecoder` for `hev1.`/`hvc1.` codecs, in the
// page and in every worker the page starts, and leaves every other codec to the real
// WebCodecs. The page itself is unchanged.
//
// Data path:
//   worker VideoDecoder.decode(chunk) -> MessagePort -> main thread
//   main thread writes the access unit into a shared ring (host-created, read-write)
//   and posts {decode, off, len}; the host decodes on its own thread straight from it
//   host copies the picture into one of a few shared slots (read-only) and posts {done}
//   main thread builds `new VideoFrame(slot, {format: "I444"})` (one copy), frees the
//   slot, and transfers the frame to the worker's output callback.
(() => {
  "use strict";
  const webview = globalThis.chrome?.webview;
  if (!webview || globalThis.__hevcShim) {
    return;
  }
  const MAGIC = "__hevcShimPort";

  // --- The `VideoDecoder` both contexts install. Self-contained: serialized into workers.
  function installDecoder(scope, openSession) {
    const Real = scope.VideoDecoder;
    const hevc = (codec) =>
      typeof codec === "string" && /^(hev1|hvc1)\./.test(codec);
    const abort = () => new DOMException("Aborted", "AbortError");

    class VideoDecoder extends EventTarget {
      #init;
      #real = null;
      #session = null;
      #state = "unconfigured";
      #queue = 0;
      #seq = 0;
      #cutoff = 0;
      #flushes = [];
      #ondequeue = null;

      constructor(init) {
        super();
        if (typeof init?.output !== "function" || typeof init?.error !== "function") {
          throw new TypeError("VideoDecoder needs output and error callbacks");
        }
        this.#init = init;
      }

      get state() {
        return this.#real ? this.#real.state : this.#state;
      }

      get decodeQueueSize() {
        return this.#real ? this.#real.decodeQueueSize : this.#queue;
      }

      get ondequeue() {
        return this.#ondequeue;
      }

      set ondequeue(handler) {
        if (this.#ondequeue) {
          this.removeEventListener("dequeue", this.#ondequeue);
        }
        this.#ondequeue = handler;
        if (handler) {
          this.addEventListener("dequeue", handler);
        }
      }

      configure(config) {
        if (this.state === "closed") {
          throw new DOMException("The decoder is closed", "InvalidStateError");
        }
        if (hevc(config?.codec)) {
          if (this.#real) {
            this.#real.close();
            this.#real = null;
          }
          if (!this.#session) {
            this.#session = openSession({
              done: (seq, frame) => this.#done(seq, frame),
              error: (message) =>
                this.#fail(new DOMException(message, "EncodingError")),
              flushed: () => this.#flushes.shift()?.resolve(),
            });
          }
          this.#session.configure({ codec: config.codec });
          this.#state = "configured";
          return;
        }
        if (this.#session) {
          this.#drop();
        }
        if (!this.#real) {
          this.#real = new Real({
            output: (frame) => this.#init.output(frame),
            error: (e) => this.#init.error(e),
          });
          this.#real.addEventListener("dequeue", () =>
            this.dispatchEvent(new Event("dequeue")),
          );
        }
        this.#real.configure(config);
      }

      decode(chunk) {
        if (this.#real) {
          this.#real.decode(chunk);
          return;
        }
        if (this.#state !== "configured") {
          throw new DOMException("The decoder is not configured", "InvalidStateError");
        }
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        this.#queue++;
        this.#session.decode(++this.#seq, data, chunk.timestamp, chunk.type === "key");
      }

      flush() {
        if (this.#real) {
          return this.#real.flush();
        }
        if (this.#state !== "configured") {
          return Promise.reject(
            new DOMException("The decoder is not configured", "InvalidStateError"),
          );
        }
        return new Promise((resolve, reject) => {
          this.#flushes.push({ resolve, reject });
          this.#session.flush();
        });
      }

      reset() {
        if (this.#real) {
          this.#real.reset();
          return;
        }
        if (this.#state === "closed") {
          throw new DOMException("The decoder is closed", "InvalidStateError");
        }
        this.#cutoff = this.#seq;
        this.#queue = 0;
        for (const flush of this.#flushes.splice(0)) {
          flush.reject(abort());
        }
        this.#session?.reset();
        this.#state = "unconfigured";
      }

      close() {
        if (this.#real) {
          this.#real.close();
          return;
        }
        this.#drop();
        this.#state = "closed";
      }

      /** Leave host mode: nothing the host still sends for this decoder is delivered. */
      #drop() {
        this.#cutoff = this.#seq;
        this.#queue = 0;
        for (const flush of this.#flushes.splice(0)) {
          flush.reject(abort());
        }
        this.#session?.close();
        this.#session = null;
      }

      #done(seq, frame) {
        if (seq <= this.#cutoff || this.#state === "closed") {
          frame?.close();
          return;
        }
        this.#queue--;
        this.dispatchEvent(new Event("dequeue"));
        if (frame) {
          this.#init.output(frame);
        }
      }

      #fail(error) {
        if (this.#state === "closed") {
          return;
        }
        this.#drop();
        this.#state = "closed";
        this.#init.error(error);
      }

      static isConfigSupported(config) {
        if (hevc(config?.codec)) {
          return Promise.resolve({ supported: true, config: { ...config } });
        }
        return Real.isConfigSupported(config);
      }
    }

    Object.defineProperty(scope, "VideoDecoder", {
      value: VideoDecoder,
      writable: true,
      configurable: true,
    });
    Object.defineProperty(scope, "__realVideoDecoder", { value: Real });
  }

  // --- Inside a worker: sessions are opened on the main thread over a private port.
  function workerMain(magic, install) {
    let port = null;
    let nextId = 1;
    const pending = [];
    const sessions = new Map();
    const post = (message, transfer = []) => {
      if (port) {
        port.postMessage(message, transfer);
      } else {
        pending.push([message, transfer]);
      }
    };
    // The port is the first message the main thread posts, ahead of anything the
    // page posts, and this listener is registered ahead of the page's own.
    self.addEventListener("message", (event) => {
      const given = event.data?.[magic];
      if (!given) {
        return;
      }
      event.stopImmediatePropagation();
      port = given;
      port.onmessage = ({ data: m }) => {
        const session = sessions.get(m.id);
        if (!session) {
          m.frame?.close();
          return;
        }
        if (m.t === "done") {
          session.done(m.seq, m.frame ?? null);
        } else if (m.t === "error") {
          session.error(m.message);
        } else if (m.t === "flushed") {
          session.flushed();
        }
      };
      for (const [message, transfer] of pending.splice(0)) {
        port.postMessage(message, transfer);
      }
    });
    install(self, (callbacks) => {
      const id = nextId++;
      sessions.set(id, callbacks);
      post({ t: "open", id });
      return {
        configure: (config) => post({ t: "configure", id, config }),
        decode: (seq, data, ts, key) =>
          post({ t: "decode", id, seq, data, ts, key }, [data.buffer]),
        flush: () => post({ t: "flush", id }),
        reset: () => post({ t: "reset", id }),
        close: () => {
          post({ t: "close", id });
          sessions.delete(id);
        },
      };
    });
  }

  const workerSource = `const MAGIC = ${JSON.stringify(MAGIC)};
${installDecoder.toString()}
${workerMain.toString()}
workerMain(MAGIC, installDecoder);
`;

  // --- Main thread: the host bridge.
  const RING_ALIGN = 64;
  const send = (message) => webview.postMessage(JSON.stringify(message));
  let ring = null;
  let ringBytes = null;
  let head = 0;
  const spans = []; // in-flight access units in ring order: {gseq, start, end, done}
  const spanOf = new Map(); // gseq -> span
  const outbox = []; // messages to the host, in order; decodes wait for ring space
  const slots = new Map(); // slot -> ArrayBuffer
  const sessions = new Map(); // id -> {callbacks, lseqOf: Map(gseq -> local seq)}
  let nextSession = 1;
  let gseq = 0;

  const stats = {
    frames: 0,
    decodeUs: [],
    copyUs: [],
    frameMs: [],
    bytesIn: 0,
    ringWaits: 0,
  };

  function place(len) {
    if (spans.length === 0) {
      head = 0;
      return len <= ring.byteLength ? 0 : -1;
    }
    // Occupied runs forward from the oldest span to `head`; it has wrapped once the
    // newest span starts before the oldest.
    const tail = spans[0].start;
    if (spans[spans.length - 1].start >= tail) {
      if (head + len <= ring.byteLength) {
        return head;
      }
      return len <= tail ? 0 : -1;
    }
    return head + len <= tail ? head : -1;
  }

  function pump() {
    while (outbox.length > 0) {
      const message = outbox[0];
      if (message.data) {
        if (!ring) {
          return;
        }
        const len = message.data.byteLength;
        if (len > ring.byteLength) {
          outbox.shift();
          sessions.get(message.id)?.callbacks.error(
            `an access unit of ${len} bytes does not fit the ${ring.byteLength}-byte ring`,
          );
          continue;
        }
        const off = place(len);
        if (off < 0) {
          stats.ringWaits++;
          return;
        }
        ringBytes.set(message.data, off);
        head = (off + len + RING_ALIGN - 1) & ~(RING_ALIGN - 1);
        const span = { gseq: message.seq, start: off, end: off + len, done: false };
        spans.push(span);
        spanOf.set(message.seq, span);
        stats.bytesIn += len;
        outbox.shift();
        send({ t: "decode", id: message.id, seq: message.seq, off, len, ts: message.ts, key: message.key });
      } else {
        outbox.shift();
        send(message);
      }
    }
  }

  function release(seq) {
    const span = spanOf.get(seq);
    if (!span) {
      return;
    }
    spanOf.delete(seq);
    span.done = true;
    while (spans.length > 0 && spans[0].done) {
      spans.shift();
    }
  }

  const COLOR = (space) =>
    space
      ? {
          primaries: space.primaries ?? null,
          transfer: space.transfer ?? null,
          matrix: space.matrix ?? null,
          fullRange: space.fullRange ?? null,
        }
      : undefined;

  function onHost(m) {
    if (m.t === "done") {
      release(m.seq);
      const session = sessions.get(m.id);
      let frame = null;
      if (m.frame) {
        const f = m.frame;
        const buffer = slots.get(f.slot);
        try {
          if (session && buffer) {
            const t0 = performance.now();
            frame = new VideoFrame(new Uint8Array(buffer, 0, f.bytes), {
              format: f.format,
              codedWidth: f.w,
              codedHeight: f.h,
              timestamp: f.ts,
              colorSpace: COLOR(f.space),
            });
            stats.frameMs.push(performance.now() - t0);
            stats.frames++;
          }
        } catch (e) {
          session?.callbacks.error(`VideoFrame from the host's picture: ${e}`);
        } finally {
          send({ t: "free", slot: f.slot });
        }
      }
      if (m.decodeUs !== undefined) {
        stats.decodeUs.push(m.decodeUs);
      }
      if (m.copyUs !== undefined) {
        stats.copyUs.push(m.copyUs);
      }
      pump();
      if (!session) {
        frame?.close();
        return;
      }
      const lseq = session.lseqOf.get(m.seq);
      session.lseqOf.delete(m.seq);
      session.callbacks.done(lseq ?? 0, frame);
    } else if (m.t === "error") {
      sessions.get(m.id)?.callbacks.error(m.message);
    } else if (m.t === "flushed") {
      sessions.get(m.id)?.callbacks.flushed();
    } else if (m.t === "retire") {
      const buffer = slots.get(m.slot);
      slots.delete(m.slot);
      if (buffer) {
        webview.releaseBuffer(buffer);
      }
    }
  }

  webview.addEventListener("message", (event) => {
    const m = event.data;
    if (m && typeof m === "object" && typeof m.t === "string") {
      onHost(m);
    }
  });
  webview.addEventListener("sharedbufferreceived", (event) => {
    const meta = event.additionalData;
    const buffer = event.getBuffer();
    if (meta?.kind === "in") {
      ring = buffer;
      ringBytes = new Uint8Array(buffer);
      pump();
    } else if (meta?.kind === "slot") {
      slots.set(meta.slot, buffer);
    }
  });

  function openSession(callbacks) {
    const id = nextSession++;
    const session = { callbacks, lseqOf: new Map() };
    sessions.set(id, session);
    const dropQueued = () => {
      for (let i = outbox.length - 1; i >= 0; i--) {
        if (outbox[i].id === id && outbox[i].data) {
          outbox.splice(i, 1);
        }
      }
    };
    return {
      configure: (config) => {
        outbox.push({ t: "configure", id, codec: config.codec });
        pump();
      },
      decode: (seq, data, ts, key) => {
        const g = ++gseq;
        session.lseqOf.set(g, seq);
        outbox.push({ id, seq: g, data, ts, key });
        pump();
      },
      flush: () => {
        outbox.push({ t: "flush", id });
        pump();
      },
      reset: () => {
        dropQueued();
        outbox.push({ t: "reset", id });
        pump();
      },
      close: () => {
        dropQueued();
        sessions.delete(id);
        outbox.push({ t: "close", id });
        pump();
      },
    };
  }

  // A worker's private port: its sessions, opened here on its behalf.
  function serve(port) {
    const mine = new Map();
    port.onmessage = ({ data: m }) => {
      if (m.t === "open") {
        mine.set(
          m.id,
          openSession({
            done: (seq, frame) =>
              port.postMessage({ t: "done", id: m.id, seq, frame }, frame ? [frame] : []),
            error: (message) => port.postMessage({ t: "error", id: m.id, message }),
            flushed: () => port.postMessage({ t: "flushed", id: m.id }),
          }),
        );
        return;
      }
      const session = mine.get(m.id);
      if (!session) {
        return;
      }
      if (m.t === "configure") {
        session.configure(m.config);
      } else if (m.t === "decode") {
        session.decode(m.seq, m.data, m.ts, m.key);
      } else if (m.t === "flush") {
        session.flush();
      } else if (m.t === "reset") {
        session.reset();
      } else if (m.t === "close") {
        session.close();
        mine.delete(m.id);
      }
    };
  }

  const blobUrl = (source) =>
    URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
  let shimUrl = null;

  // A worker starts from a blob entry that imports the shim, then its own script, so
  // its `location` is the blob's: its own script's `import.meta.url` is unchanged, but
  // a relative `fetch` from it would resolve against the blob.
  const RealWorker = globalThis.Worker;
  function Worker(url, options) {
    if (!new.target) {
      throw new TypeError("Failed to construct 'Worker': Please use the 'new' operator");
    }
    shimUrl ??= blobUrl(workerSource);
    const script = new URL(url, location.href).href;
    const entry =
      options?.type === "module"
        ? blobUrl(`import ${JSON.stringify(shimUrl)};\nimport ${JSON.stringify(script)};\n`)
        : blobUrl(`importScripts(${JSON.stringify(shimUrl)}, ${JSON.stringify(script)});\n`);
    const worker = new RealWorker(entry, options);
    const channel = new MessageChannel();
    worker.postMessage({ [MAGIC]: channel.port2 }, [channel.port2]);
    serve(channel.port1);
    return worker;
  }
  Worker.prototype = RealWorker.prototype;
  Object.defineProperty(globalThis, "Worker", { value: Worker, writable: true, configurable: true });

  installDecoder(globalThis, openSession);

  Object.defineProperty(globalThis, "__hevcShim", {
    value: Object.freeze({
      stats: () => ({ ...stats, slots: slots.size, ring: ring?.byteLength ?? 0 }),
      log: (text) => send({ t: "log", text: String(text) }),
    }),
  });

  send({ t: "hello" });
})();
