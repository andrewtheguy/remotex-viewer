#!/usr/bin/env bash
# Build remotex-viewer's Windows installer from a release tag and attach it to a release
# of the same name in the operator's private repository, andrewtheguy/remotex-viewer-releases.
#
# The release workflow never builds the viewer: on Windows it links FFmpeg's HEVC
# decoder, whose licence keeps it out of public release artifacts, as it keeps the
# `apple-hp-media` feature out of them (publish-full-image.sh is that feature's private
# build). The repository must stay private; the script refuses one that is not.
#
# It builds a tag and nothing else, from `git archive` of that tag in this checkout,
# and only one GitHub also has at the same commit. The build runs on windows-ci-build
# (build-windows-viewer.ps1, the tag's own copy), which has no `gh`: the libavcodec
# archive is downloaded here — the release of libavcodec-hevc-prebuilt-archives named
# by the tag the viewer's Cargo.toml pins — checked against that release's SHA256SUMS,
# and sent there with the source.
#
# Log in to gh first, with an account that can read
# andrewtheguy/libavcodec-hevc-prebuilt-archives and write
# andrewtheguy/remotex-viewer-releases:
#
#   gh auth login
#
#   packaging/publish-windows-viewer.sh TAG
#
#   TAG  the tag to build, e.g. v0.0.279
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

releases=andrewtheguy/remotex-viewer-releases
hevc_archives=andrewtheguy/libavcodec-hevc-prebuilt-archives
windows=windows-ci-build
stage='C:\ci-workspaces\remotex-viewer-release'
stage_fwd='C:/ci-workspaces/remotex-viewer-release'
built='C:/ci-workspaces/remotex-viewer-release/dist/remotex-viewer-windows-x86_64.msi'

[ $# -eq 1 ] && [ "${1#-}" = "$1" ] || { echo "usage: $0 TAG" >&2; exit 2; }
tag="$1"

commit="$(git rev-parse --verify --quiet "refs/tags/${tag}^{commit}")" \
  || { echo "this checkout has no tag ${tag}: git fetch --tags" >&2; exit 1; }
remote="$(git ls-remote origin "refs/tags/${tag}^{}" "refs/tags/${tag}" | awk '{print $1}' | tail -n 1)"
[ -n "$remote" ] || { echo "origin has no tag ${tag}: push it first" >&2; exit 1; }
remote_commit="$(git rev-parse --verify --quiet "${remote}^{commit}" 2>/dev/null || echo "$remote")"
[ "$remote_commit" = "$commit" ] \
  || { echo "${tag} is ${commit} here and ${remote_commit} on origin" >&2; exit 1; }

version="$(git show "${commit}:Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1)"
[ "v${version}" = "$tag" ] \
  || { echo "${tag} builds remotex ${version}; a tag is v<its version>" >&2; exit 1; }

# Before anything is built.
visibility="$(gh repo view "$releases" --json visibility --jq .visibility)" \
  || { echo "gh cannot see ${releases}: gh auth login, with an account that can" >&2; exit 1; }
[ "$visibility" = PRIVATE ] \
  || { echo "${releases} is ${visibility}, not private: not publishing to it" >&2; exit 1; }
if gh release view "$tag" --repo "$releases" >/dev/null 2>&1; then
  echo "${releases} already has a release ${tag}" >&2
  exit 1
fi

# The archive release the tag's viewer pins: libavcodec-hevc-prebuilt tags its source
# and its archives' release alike.
pin="$(git show "${commit}:crates/remotex-viewer/Cargo.toml" \
  | sed -n 's/^avcodec-hevc-sys = .*tag = "\([^"]*\)".*$/\1/p')"
[ -n "$pin" ] || { echo "${tag}'s viewer pins no libavcodec-hevc-prebuilt tag" >&2; exit 1; }

work="$repo_root/tmp/windows-viewer"
mkdir -p "$work"
exec 9>"$work/lock"
flock -n 9 || { echo "another Windows viewer build is running in $work" >&2; exit 1; }
out="$work/$tag"
rm -rf "$out"
mkdir -p "$out/hevc"

echo ">> libavcodec: ${hevc_archives} ${pin}"
gh release download "$pin" --repo "$hevc_archives" --dir "$out/hevc" \
  --pattern '*-windows-x86_64-msvc.tar.gz' --pattern SHA256SUMS \
  || { echo "gh cannot download ${hevc_archives} ${pin}: gh auth login, with an account that can" >&2; exit 1; }
(cd "$out/hevc" && sha256sum --check --ignore-missing --strict SHA256SUMS)
archive="$(ls "$out"/hevc/*-windows-x86_64-msvc.tar.gz)"
ffmpeg="$(tar -xzf "$archive" ./MANIFEST -O | sed -n 's/^ffmpeg //p')"

echo ">> staging ${tag} (${commit}) on ${windows}"
ssh "$windows" "pwsh -NoLogo -Command \"Remove-Item -Recurse -Force ${stage} -ErrorAction SilentlyContinue; New-Item -ItemType Directory ${stage}\\src, ${stage}\\hevc | Out-Null\""
git archive "$commit" \
  | ssh "$windows" "tar -xf - -C ${stage_fwd}/src"
ssh "$windows" "tar -xzf - -C ${stage_fwd}/hevc" < "$archive"

echo ">> building and checking the remotex-viewer ${version} installer"
ssh "$windows" "pwsh -NoLogo -File ${stage}\\src\\packaging\\build-windows-viewer.ps1 -Root ${stage}"

msi="remotex-viewer-${version}-windows-x86_64.msi"
scp -q "${windows}:${built}" "$out/$msi"
(cd "$out" && sha256sum "$msi" > SHA256SUMS)

echo ">> publishing ${releases} ${tag}"
gh release create "$tag" --repo "$releases" --title "remotex-viewer ${version}" --notes "$(cat <<EOF
remotex-viewer ${version}'s installer for Windows x86-64, built from andrewtheguy/remotex ${tag} (${commit}).

The installer puts remotex viewer under Program Files with a Start menu shortcut, and the Visual C++ runtime beside it. The viewer links FFmpeg ${ffmpeg}'s libavcodec (LGPL-2.1-or-later) statically, from ${hevc_archives} ${pin}, and needs the WebView2 runtime, which ships with Windows 11 and current Windows 10.
EOF
)" "$out/$msi" "$out/SHA256SUMS"

echo ">> published ${releases} ${tag}"
