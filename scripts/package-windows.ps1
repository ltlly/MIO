param(
    [ValidateSet("release", "debug")]
    [string]$Profile = "release",
    [string]$Target = ""
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectRoot
$env:CARGO_INCREMENTAL = "0"

$SourceCommit = (git rev-parse HEAD).Trim()
$BranchOut = git symbolic-ref --quiet --short HEAD 2>$null
if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($BranchOut)) {
    $SourceBranch = $BranchOut.Trim()
} else {
    $SourceBranch = "detached"
}
$SourceDirty = [bool](git status --porcelain --untracked-files=normal)

$CargoArgs = @("build", "--locked", "-p", "mio-app", "-p", "mio-cli", "--bins")
if ($Profile -eq "release") {
    $CargoArgs += "--release"
}
if ($Target -ne "") {
    $CargoArgs += @("--target", $Target)
    $BinDir = Join-Path $ProjectRoot "target\$Target\$Profile"
    $Arch = ($Target -split "-")[0]
} else {
    $BinDir = Join-Path $ProjectRoot "target\$Profile"
    $Arch = $env:PROCESSOR_ARCHITECTURE.ToLowerInvariant()
}
switch ($Arch) {
    "amd64" { $Arch = "x86_64" }
    "arm64" { $Arch = "aarch64" }
}

$PrevRustFlags = $env:RUSTFLAGS
if ([string]::IsNullOrWhiteSpace($PrevRustFlags)) {
    $env:RUSTFLAGS = "-C target-feature=+crt-static"
} elseif ($PrevRustFlags -notmatch "\+crt-static") {
    $env:RUSTFLAGS = "$PrevRustFlags -C target-feature=+crt-static"
}
try {
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
} finally {
    $env:RUSTFLAGS = $PrevRustFlags
}

if ((git rev-parse HEAD).Trim() -ne $SourceCommit) {
    throw "Source commit changed during the build; distribution was not updated."
}
if (git status --porcelain --untracked-files=normal) {
    $SourceDirty = $true
}

$DistDir = Join-Path $ProjectRoot "dist\windows-$Arch"
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
Copy-Item -Force (Join-Path $BinDir "mio.exe") (Join-Path $DistDir "mio.exe")
Copy-Item -Force (Join-Path $BinDir "mio-cli.exe") (Join-Path $DistDir "mio-cli.exe")

$Sha256 = (Get-FileHash -Algorithm SHA256 (Join-Path $DistDir "mio.exe")).Hash.ToLowerInvariant()
$Manifest = [ordered]@{
    app          = "mio.exe"
    platform     = "windows"
    architecture = $Arch
    profile      = $Profile
    branch       = $SourceBranch
    sourceCommit = $SourceCommit
    sourceDirty  = $SourceDirty
    binarySha256 = $Sha256
    packagedAt   = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
}
$ManifestJson = ($Manifest | ConvertTo-Json -Depth 4) + "`n"
[System.IO.File]::WriteAllText((Join-Path $DistDir "preview-build.json"), $ManifestJson, [System.Text.UTF8Encoding]::new($false))
Write-Output $DistDir
