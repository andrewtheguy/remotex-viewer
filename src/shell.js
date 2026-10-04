// Injected into every document at the gateway's origin in a gateway window (main
// frame only).
//
// The page is the gateway's, unchanged. This makes the web platform it already uses
// do what it promises in a window of its own:
//
//   - element full screen fills the monitor (the shell resizes the window);
//   - while the shell holds the keyboard (the remote surface focused, in front, in
//     any mode), keys arrive here from the OS and go to the focused element as the key
//     events the page already listens for, and a held Escape leaves full screen, as
//     it does in Chromium;
//   - `navigator.clipboard` is the system clipboard, answered by the shell;
//   - `window.resizeTo` sizes this window, and the window reports itself as the app
//     window it is, which is what shows the page's **Size to** item.
//
// Shell -> page is `__remotexShell.receive(message)`; page -> shell is `ipc.postMessage`.
(() => {
  "use strict";
  const ipc = globalThis.ipc;
  if (!ipc || globalThis.__remotexShell) {
    return;
  }
  const send = (message) => ipc.postMessage(JSON.stringify(message));

  // --- Full screen: the page's own, which the shell follows with the window.
  const reportFullscreen = () =>
    send({ t: "shell.fullscreen", on: document.fullscreenElement !== null });
  document.addEventListener("fullscreenchange", reportFullscreen);

  // --- Whether a key is meant for the remote: its surface is the page's
  // `role="application"` element, and a key typed into anything else (a panel's text
  // box, the login form) stays local.
  let surface = false;
  const reportSurface = () => {
    const now =
      document.hasFocus() && document.activeElement?.getAttribute?.("role") === "application";
    if (now !== surface) {
      surface = now;
      send({ t: "shell.surface", focused: now });
    }
  };
  document.addEventListener("focusin", reportSurface, true);
  document.addEventListener("focusout", () => setTimeout(reportSurface), true);
  window.addEventListener("focus", reportSurface);
  window.addEventListener("blur", reportSurface);

  // --- Keys the shell took from the OS. Flags come from the keys held here, which
  // while the shell holds the keyboard are all of them; Caps Lock's state is learnt
  // from the last real event before that, since the shell's keys never change the
  // local one.
  const MODIFIERS = {
    ctrlKey: ["ControlLeft", "ControlRight"],
    shiftKey: ["ShiftLeft", "ShiftRight"],
    altKey: ["AltLeft", "AltRight"],
    metaKey: ["MetaLeft", "MetaRight"],
  };
  const MODIFIER_NAMES = { Control: "ctrlKey", Shift: "shiftKey", Alt: "altKey", Meta: "metaKey" };
  const ESCAPE_HOLD_MS = 1500;
  const held = new Set();
  let capturing = false;
  let caps = false;
  let escapeSince = 0;
  const flags = () => {
    const now = {};
    for (const [flag, codes] of Object.entries(MODIFIERS)) {
      now[flag] = codes.some((c) => held.has(c));
    }
    return now;
  };
  for (const type of ["keydown", "pointerdown", "pointermove"]) {
    window.addEventListener(
      type,
      (e) => {
        if (e.isTrusted && !capturing) {
          caps = e.getModifierState("CapsLock");
        }
      },
      { capture: true, passive: true },
    );
  }
  // The OS never saw the modifiers the shell took, so a real click says none are
  // down — which the page reads as their release. While the shell holds the keys,
  // pointer events carry the modifiers held here instead, before any listener of the
  // page's sees them.
  const stampModifiers = (e) => {
    if (!capturing || !e.isTrusted) {
      return;
    }
    const now = flags();
    for (const [flag, on] of Object.entries(now)) {
      Object.defineProperty(e, flag, { value: on });
    }
    const real = e.getModifierState.bind(e);
    Object.defineProperty(e, "getModifierState", {
      value: (name) =>
        name in MODIFIER_NAMES
          ? now[MODIFIER_NAMES[name]]
          : name === "CapsLock"
            ? caps
            : name === "AltGraph"
              ? false
              : real(name),
    });
  };
  for (const type of [
    "pointerdown", "pointermove", "pointerup", "mousedown", "mousemove", "mouseup",
    "click", "dblclick", "auxclick", "contextmenu", "wheel",
    "touchstart", "touchmove", "touchend", "touchcancel",
  ]) {
    window.addEventListener(type, stampModifiers, { capture: true, passive: true });
  }
  function key(code, pressed) {
    const repeat = pressed && held.has(code);
    if (pressed) {
      held.add(code);
      if (!repeat && code === "CapsLock") {
        caps = !caps;
      }
    } else {
      held.delete(code);
    }
    const init = {
      ...flags(),
      code,
      key: "Unidentified",
      repeat,
      bubbles: true,
      cancelable: true,
      composed: true,
      modifierCapsLock: caps,
    };
    (document.activeElement ?? document.body).dispatchEvent(
      new KeyboardEvent(pressed ? "keydown" : "keyup", init),
    );
    if (code === "Escape") {
      if (!pressed) {
        escapeSince = 0;
      } else if (!repeat) {
        escapeSince = performance.now();
      } else if (escapeSince && performance.now() - escapeSince >= ESCAPE_HOLD_MS) {
        escapeSince = 0;
        document.exitFullscreen?.().catch(() => {});
      }
    }
    // The surface can lose focus with no event, when its element leaves the document.
    reportSurface();
  }

  // --- The clipboard.
  const pending = new Map();
  let nextId = 1;
  const ask = (message) =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, { resolve, reject });
      send({ ...message, id });
    });
  const clipboard = navigator.clipboard;
  if (clipboard) {
    Object.defineProperty(clipboard, "readText", {
      value: () => ask({ t: "shell.clipboardRead" }),
      configurable: true,
      writable: true,
    });
    Object.defineProperty(clipboard, "writeText", {
      value: (text) => ask({ t: "shell.clipboardWrite", text: String(text) }).then(() => {}),
      configurable: true,
      writable: true,
    });
  }

  // --- The window. `resizeTo` takes an outer size, and what the page wants is the
  // inner one it computed that from, so the frame it added comes off again here and
  // the shell adds its own; the shell works in physical pixels.
  Object.defineProperty(window, "resizeTo", {
    value: (w, h) => {
      const dpr = window.devicePixelRatio || 1;
      const inner = (outer, frame) => Math.max(1, Math.ceil((Number(outer) - frame) * dpr));
      send({
        t: "shell.resizeTo",
        w: inner(w, window.outerWidth - window.innerWidth),
        h: inner(h, window.outerHeight - window.innerHeight),
      });
    },
    configurable: true,
    writable: true,
  });
  const realMatchMedia = window.matchMedia.bind(window);
  const APP_MODE = { standalone: "(width >= 0px)", browser: "(width < 0px)" };
  Object.defineProperty(window, "matchMedia", {
    value: (query) =>
      realMatchMedia(
        String(query).replace(
          /\(\s*display-mode\s*:\s*([a-z-]+)\s*\)/g,
          (all, mode) => APP_MODE[mode] ?? all,
        ),
      ),
    configurable: true,
    writable: true,
  });

  Object.defineProperty(globalThis, "__remotexShell", {
    value: Object.freeze({
      receive(m) {
        if (m.t === "key") {
          key(m.code, m.pressed);
        } else if (m.t === "capture") {
          capturing = m.on;
          held.clear();
          escapeSince = 0;
        } else if (m.t === "clipboard") {
          const request = pending.get(m.id);
          if (request) {
            pending.delete(m.id);
            if (m.ok) {
              request.resolve(m.text);
            } else {
              request.reject(new DOMException(m.error, "NotAllowedError"));
            }
          }
        }
      },
    }),
  });

  // A new document is never in full screen, whatever the last one left the window in.
  reportFullscreen();
})();
