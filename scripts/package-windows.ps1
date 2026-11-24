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

function Resolve-SignToolPath {
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kitsRoot) {
        $candidate = Get-ChildItem -Path $kitsRoot -Filter signtool.exe -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\(x64|arm64)\\signtool\.exe$' } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($candidate) { return $candidate.FullName }
    }
    return $null
}

$Signed = $false
$CertPfx = $env:MIO_WINDOWS_CERT_PFX
$CertPfxB64 = $env:MIO_WINDOWS_CERT_PFX_BASE64
$CertThumbprint = $env:MIO_WINDOWS_CERT_THUMBPRINT
$CertPassword = $env:MIO_WINDOWS_CERT_PASSWORD
$TimestampUrl = if ([string]::IsNullOrWhiteSpace($env:MIO_WINDOWS_TIMESTAMP_URL)) {
    "http://timestamp.digicert.com"
} else {
    $env:MIO_WINDOWS_TIMESTAMP_URL.Trim()
}

if (-not [string]::IsNullOrWhiteSpace($CertPfx) -or
    -not [string]::IsNullOrWhiteSpace($CertPfxB64) -or
    -not [string]::IsNullOrWhiteSpace($CertThumbprint)) {
    $SignTool = Resolve-SignToolPath
    if (-not $SignTool) {
        throw "Authenticode certificate configured, but signtool.exe was not found on PATH or in Windows Kits 10."
    }
    $TempPfx = $null
    $EffectivePfx = $CertPfx
    try {
        if ([string]::IsNullOrWhiteSpace($EffectivePfx) -and -not [string]::IsNullOrWhiteSpace($CertPfxB64)) {
            $TempPfx = Join-Path ([System.IO.Path]::GetTempPath()) ("mio-sign-" + [System.Guid]::NewGuid().ToString("N") + ".pfx")
            [System.IO.File]::WriteAllBytes($TempPfx, [System.Convert]::FromBase64String($CertPfxB64.Trim()))
            $EffectivePfx = $TempPfx
        }
        foreach ($BinName in @("mio.exe", "mio-cli.exe")) {
            $TargetExe = Join-Path $DistDir $BinName
            $SignArgs = @("sign", "/fd", "SHA256", "/tr", $TimestampUrl, "/td", "SHA256")
            if (-not [string]::IsNullOrWhiteSpace($EffectivePfx)) {
                $SignArgs += @("/f", $EffectivePfx)
                if (-not [string]::IsNullOrWhiteSpace($CertPassword)) {
                    $SignArgs += @("/p", $CertPassword)
                }
            } else {
                $SignArgs += @("/sha1", $CertThumbprint.Trim())
            }
            $SignArgs += $TargetExe
            & $SignTool @SignArgs
            if ($LASTEXITCODE -ne 0) {
                throw "signtool sign failed for $TargetExe with exit code $LASTEXITCODE"
            }
            & $SignTool verify /pa $TargetExe
            if ($LASTEXITCODE -ne 0) {
                throw "signtool verify failed for $TargetExe with exit code $LASTEXITCODE"
            }
        }
        $Signed = $true
    } finally {
        if ($TempPfx -and (Test-Path -LiteralPath $TempPfx)) {
            Remove-Item -LiteralPath $TempPfx -Force -ErrorAction SilentlyContinue
        }
    }
}

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
    signed       = $Signed
    signingMode  = if ($Signed) { "authenticode" } else { "unsigned" }
    packagedAt   = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
}
$ManifestJson = ($Manifest | ConvertTo-Json -Depth 4) + "`n"
[System.IO.File]::WriteAllText((Join-Path $DistDir "preview-build.json"), $ManifestJson, [System.Text.UTF8Encoding]::new($false))
Write-Output $DistDir
