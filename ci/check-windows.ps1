# The Windows half of ci/check.sh, run on windows-ci-build in the tree it staged:
# clippy against the libavcodec prefix staged there.
#Requires -Version 7
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)
$env:CARGO_TARGET_DIR = 'C:\ci-workspaces\remotex-viewer-target'
$env:LIBAVCODEC_HEVC_PREBUILT_DIR = 'C:\ci-workspaces\libavcodec-hevc-prebuilt\dist\windows-x86_64-msvc'
& cargo clippy --locked --all-targets -- -D warnings 2>&1 | ForEach-Object { "$_" }
exit $LASTEXITCODE
