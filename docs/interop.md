# The interop protocol

The viewer shows a remotex gateway's page unchanged, and most of what it does for it is
the web platform: full screen, key events, `navigator.clipboard`, `window.resizeTo`.
Where it goes beyond that, what it provides and what the page uses of it is a versioned
protocol. remotex numbers it and defines each version, in its
[`docs/viewer.md`](https://github.com/andrewtheguy/remotex/blob/main/docs/viewer.md);
this repository implements the versions it speaks.

## Who states what

The gateway's page states the one version it speaks, in its head:

```html
<meta name="remotex-viewer-interop" content="1" />
```

The viewer speaks a list of versions, `INTEROP` in `src/webview2/hevc_shim.js`, and uses
a part of the protocol only in a document whose version is in it. A document that states
another version, or none, is left to the web platform: a gateway newer or older than the
viewer is a session without that part — the picture as VP9 rather than the Mac's HEVC —
never a broken one. Each is updated on its own, and neither refuses the other.

The page asks its questions from its scripts, which run after its head is parsed, so the
viewer reads the declaration when it is asked rather than when its scripts are injected,
which is before.

## The versions

| Version | First stated by remotex | What it is | Where |
|---|---|---|---|
| 1 | the release after 0.0.283 | HEVC on Windows: FFmpeg's HEVC decoder behind the page's `VideoDecoder`, for `hev1.`/`hvc1.` codecs, in the page and in the workers it starts | `src/webview2/` |

## A new version

remotex bumps the version when the page changes anything a version describes, and
describes the new one. The viewer then implements it and adds it to `INTEROP`, keeping
the versions older gateways still state for as long as they are in use, and the table
above gets its row.
