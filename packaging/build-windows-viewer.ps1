# The Windows half of publish-windows-viewer.sh: build remotex-viewer's installer from what
# that script staged under -Root — the tag's tree in src\, and the released libavcodec
# archive's prefix in hevc\ — into -Root\dist\remotex-viewer-windows-x86_64.msi, then install
# it, check what it installed and remove it again.
#
# Needs cargo, the MSVC build tools (whose VC\Redist holds the runtime the package carries)
# and WiX 5 on PATH, and an elevated session: the package installs per machine.
#Requires -Version 7
param([Parameter(Mandatory)][string]$Root)
$ErrorActionPreference = 'Stop'

if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    throw 'wix is not on PATH: dotnet tool install --global wix --version 5.0.2'
}
$uiExt = 'WixToolset.UI.wixext'
& wix extension add -g "$uiExt/5.0.2"
if ($LASTEXITCODE -ne 0) { throw "wix extension add $uiExt failed (exit $LASTEXITCODE)" }

$env:LIBAVCODEC_HEVC_PREBUILT_DIR = Join-Path $Root 'hevc'
# Apart from the target directory day-to-day builds use, so a release never picks up
# what a local build left, and dependencies stay compiled between releases.
$env:CARGO_TARGET_DIR = 'C:\ci-workspaces\cargo-target-viewer-release'
Set-Location (Join-Path $Root 'src')

$metadata = & cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed (exit $LASTEXITCODE)" }
$version = ($metadata.packages | Where-Object { $_.name -eq 'remotex-viewer' }).version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "remotex-viewer $version is not an MSI version (x.y.z)" }

# Cargo reruns a build script when its inputs change, and the archive's path does not:
# cleaning that one crate makes it link the archive staged now.
& cargo clean --release -p libavcodec-hevc-prebuilt-sys 2>&1 | ForEach-Object { "$_" }
& cargo build -p remotex-viewer --release --locked 2>&1 | ForEach-Object { "$_" }
if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
$exe = Join-Path $env:CARGO_TARGET_DIR 'release\remotex-viewer.exe'

# The runtime from the build tools that linked the executable: VC\Redist holds the same
# version as VC\Tools.
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath
$crt = Get-ChildItem "$vs\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT" -Directory |
    Where-Object { $_.Parent.Parent.Name -match '^\d+(\.\d+)+$' } |
    Sort-Object { [version]$_.Parent.Parent.Name } | Select-Object -Last 1
if (-not $crt) { throw "no x64 Visual C++ runtime under $vs\VC\Redist\MSVC" }
Write-Host ">> Visual C++ runtime from $($crt.FullName)"

$stage = Join-Path $Root 'stage'
New-Item -ItemType Directory -Force -Path $stage, (Join-Path $Root 'dist') | Out-Null
Copy-Item $exe $stage
Copy-Item (Join-Path $crt.FullName 'vcruntime140.dll'), (Join-Path $crt.FullName 'vcruntime140_1.dll') $stage

$msi = Join-Path $Root 'dist\remotex-viewer-windows-x86_64.msi'
Write-Host ">> building the MSI for remotex-viewer $version"
& wix build -arch x64 -ext $uiExt -d "Version=$version" -d "Stage=$stage" `
    -d "Icon=$(Resolve-Path 'crates\remotex-viewer\icons\remotex-viewer.ico')" `
    -o $msi packaging\windows\remotex-viewer.wxs
if ($LASTEXITCODE -ne 0) { throw "wix build failed (exit $LASTEXITCODE)" }

# Install it, see that it put the viewer, its runtime and its shortcut where they go, and
# take it away again. The viewer itself is a window, which a session with no desktop cannot
# show, so it is not started.
$installed = Join-Path $env:ProgramFiles 'remotex-viewer'
$shortcut = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\remotex viewer.lnk'
if (Test-Path $installed) { throw "$installed exists before the install: remove the installed viewer first" }
function Invoke-Msiexec([string[]] $Arguments, [string] $What) {
    $log = Join-Path $Root "msiexec-$What.log"
    $p = Start-Process msiexec -ArgumentList ($Arguments + @('/qn', '/norestart', '/l*v', $log)) -Wait -PassThru
    if ($p.ExitCode -ne 0) {
        Get-Content $log | Select-Object -Last 40
        throw "msiexec $What exited $($p.ExitCode)"
    }
}
Write-Host '>> installing it'
Invoke-Msiexec @('/i', $msi) 'install'
foreach ($file in 'remotex-viewer.exe', 'vcruntime140.dll', 'vcruntime140_1.dll') {
    if (-not (Test-Path (Join-Path $installed $file))) { throw "the install lacks $file" }
}
if (-not (Test-Path $shortcut)) { throw "the install made no $shortcut" }
Write-Host '>> removing it'
Invoke-Msiexec @('/x', $msi) 'remove'
if (Test-Path $installed) { throw "$installed is left after the removal" }
if (Test-Path $shortcut) { throw "$shortcut is left after the removal" }
Write-Host ">> wrote $msi ($([math]::Round((Get-Item $msi).Length / 1MB, 1)) MB), installed and removed cleanly"
