#!/usr/bin/env bash
# Clippy on the two platforms the viewer is for, with this working tree as it is:
# windows-ci-build (where the HEVC path builds, against the libavcodec prefix staged
# there: check-windows.ps1) and macvm. Linux is out of the viewer's scope, so nothing
# is built here.
#
#   ci/check.sh [windows|mac]...   both when none is named
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

windows=windows-ci-build
win_src='C:\ci-workspaces\remotex-viewer'
win_src_fwd='C:/ci-workspaces/remotex-viewer'
mac=macvm
mac_src='codes/staging-area/remotex-viewer'

# The tree's files, committed or not, and nothing ignored; a deleted one is skipped.
files() {
  git ls-files -z --cached --others --exclude-standard
}

check_windows() {
  echo ">> clippy on ${windows}"
  ssh "$windows" "pwsh -NoLogo -Command \"Remove-Item -Recurse -Force ${win_src} -ErrorAction SilentlyContinue; New-Item -ItemType Directory ${win_src} | Out-Null\""
  files | tar --null -T - --ignore-failed-read -cf - 2>/dev/null | ssh "$windows" "tar -xf - -C ${win_src_fwd}"
  ssh "$windows" "pwsh -NoLogo -File ${win_src}\\ci\\check-windows.ps1"
}

check_mac() {
  echo ">> clippy on ${mac}"
  ssh "$mac" "mkdir -p ${mac_src}"
  rsync -a --delete --exclude .git --exclude /target --exclude tmp/ ./ "${mac}:${mac_src}/"
  ssh "$mac" "cd ${mac_src} && PATH=\"\$HOME/.cargo/bin:/opt/homebrew/bin:\$PATH\" cargo clippy --locked --all-targets -- -D warnings"
}

[ $# -gt 0 ] || set -- windows mac
for platform in "$@"; do
  case "$platform" in
    windows) check_windows ;;
    mac) check_mac ;;
    *) echo "usage: $0 [windows|mac]..." >&2; exit 2 ;;
  esac
done
echo ">> clean: $*"
