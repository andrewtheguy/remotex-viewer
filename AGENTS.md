# Repository instructions

Keep this file to rules that change how work is performed. Design explanations and
guides belong in the [README](README.md) and [docs/](docs/).

## Workflow

- Strict no backward-compatibility or legacy paths, save the interop protocol versions
  a gateway in use still states ([The interop protocol](docs/interop.md)).
- Do not run `cargo fmt`.
- No squash merges.
- After changes, run `ci/check.sh`: clippy with `-D warnings` on `windows-ci-build` and
  on macvm. Linux is out of the viewer's scope: do not build it here.
- Put temporary files under `tmp/`. Always run local Python through `uv`.
- Errors are `anyhow`. Carry the cause: add `.context()` on the way up.
- QA against a gateway is by hand, on `windows-ci-build`'s desktop: build the viewer
  there, then hand over. Do not start it from a scheduled task or drive it over the
  DevTools port.

## Product boundaries

- The viewer is a shell around a remotex gateway's page, which it shows unchanged.
  It delivers web platform APIs the page already uses — full screen, key events,
  `navigator.clipboard`, `window.resizeTo`, and on Windows HEVC decoding — natively.
  It never implements a page feature, and the page carries no code for it. Its one
  screen of its own is the library that picks a gateway, a name and a URL each; a
  gateway's window gets no button or bar from it.
- What the viewer provides beyond the web platform is the interop protocol, which
  remotex numbers and defines (its `docs/viewer.md`). Use a part of it only in a
  document that states a version the viewer speaks (`INTEROP` in
  `src/webview2/hevc_shim.js`), and leave any other document to the web platform. Do
  not refuse a gateway over its version, and do not extend a released version: a
  change is a new version, defined in remotex first.
- The shell and the decoder are the gateway's origin's alone: a document of any other
  origin gets neither.

## Releasing

- Releases are private: the installer goes only to
  `andrewtheguy/remotex-viewer-releases`, through
  `packaging/publish-windows-viewer.sh`, from a pushed tag `v<the version in
  Cargo.toml>`. Never attach a binary to a release of this repository.
