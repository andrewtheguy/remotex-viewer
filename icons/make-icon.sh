#!/usr/bin/env bash
# Rasterize icon.svg into remotex-viewer.ico, which build.rs embeds in the Windows
# executable and the viewer's windows show.
#
# The .ico is committed and this script regenerates it, so a build needs no SVG
# rasterizer: edit icon.svg, run this, commit both.
#
# Needs rsvg-convert (librsvg) and ImageMagick's `magick`. Each size is rendered from
# the SVG itself rather than scaled from a larger picture, so 16x16 is drawn at 16x16.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
svg="$here/icon.svg"
ico="$here/remotex-viewer.ico"

for tool in rsvg-convert magick; do
  command -v "$tool" >/dev/null || { echo "error: $tool not found" >&2; exit 1; }
done

# mktemp, not a path in the repo: a failed run must leave nothing behind.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The body with a 28-unit margin rather than macOS's 100: a Windows icon fills its
# square, and a small one is too small already.
sed 's|width="1024" height="1024" viewBox="0 0 1024 1024"|width="880" height="880" viewBox="72 72 880 880"|' \
  "$svg" > "$work/icon.svg"
grep -q 'viewBox="72 72 880 880"' "$work/icon.svg" || { echo "error: icon.svg's canvas is not 1024x1024" >&2; exit 1; }

# Windows' own sizes: the title bar and small icons at 100-250% scale, the taskbar
# and Alt+Tab, and 256 for Explorer's large views.
pngs=()
for size in 16 20 24 32 40 48 64 256; do
  rsvg-convert -w "$size" -h "$size" "$work/icon.svg" -o "$work/$size.png"
  pngs+=("$work/$size.png")
done

magick "${pngs[@]}" "$ico"
echo ">> wrote $ico ($(stat -c%s "$ico" 2>/dev/null || stat -f%z "$ico") bytes)"
