#!/usr/bin/env bash
# Clippy on windows-ci-build, the one platform the viewer is for, with this working
# tree as it is (the HEVC path builds against the libavcodec prefix staged there:
# check-windows.ps1). The viewer is Windows alone, so nothing is built here.
#
#   ci/check.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

[ $# -eq 0 ] || { echo "usage: $0" >&2; exit 2; }

windows=windows-ci-build
win_src='C:\ci-workspaces\remotex-viewer'
win_src_fwd='C:/ci-workspaces/remotex-viewer'

# The tree's files, committed or not, and nothing ignored; a deleted one is skipped.
# Only tar's common flags, so bsdtar (macOS's) serves as well as GNU tar.
files() {
  git ls-files -z --cached --others --exclude-standard | while IFS= read -r -d "" file; do
    [ -e "$file" ] && printf "%s\0" "$file"
  done
}

echo ">> clippy on ${windows}"
ssh "$windows" "pwsh -NoLogo -Command \"Remove-Item -Recurse -Force ${win_src} -ErrorAction SilentlyContinue; New-Item -ItemType Directory ${win_src} | Out-Null\""
files | tar --null -T - -cf - | ssh "$windows" "tar -xf - -C ${win_src_fwd}"
ssh "$windows" "pwsh -NoLogo -File ${win_src}\\ci\\check-windows.ps1"
echo ">> clean"
