# The Windows half of publish-windows-viewer.sh: build remotex-viewer.exe from what that
# script staged under -Root — the tag's tree in src\, and the released libavcodec
# archive's prefix in hevc\ — and nothing else.
param([Parameter(Mandatory)][string]$Root)
$ErrorActionPreference = 'Stop'

$env:LIBAVCODEC_HEVC_PREBUILT_DIR = Join-Path $Root 'hevc'
# Apart from the target directory day-to-day builds use, so a release never picks up
# what a local build left, and dependencies stay compiled between releases.
$env:CARGO_TARGET_DIR = 'C:\ci-workspaces\cargo-target-viewer-release'
Set-Location (Join-Path $Root 'src')

# Cargo reruns a build script when its inputs change, and the archive's path does not:
# cleaning that one crate makes it link the archive staged now.
& cargo clean --release -p libavcodec-hevc-prebuilt-sys 2>&1 | ForEach-Object { "$_" }
& cargo build -p remotex-viewer --release --locked 2>&1 | ForEach-Object { "$_" }
exit $LASTEXITCODE
