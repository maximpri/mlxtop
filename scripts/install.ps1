# SPDX-License-Identifier: MIT
# Install mlxtop on Windows (x86_64) without administrator rights:
#   irm https://raw.githubusercontent.com/maximpri/mlxtop/main/scripts/install.ps1 | iex
# Downloads the release zip, checks it against the published SHA256SUMS, and
# places mlxtop.exe in %LOCALAPPDATA%\Programs\mlxtop, which is added to the
# user PATH. Environment overrides: MLXTOP_VERSION (for example 3.0.0),
# MLXTOP_INSTALL_DIR, and MLXTOP_NO_MODIFY_PATH=1.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

function Install-Mlxtop {
    if (-not [Environment]::Is64BitOperatingSystem) {
        throw 'mlxtop requires 64-bit Windows.'
    }
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $repository = 'https://github.com/maximpri/mlxtop'
    $target = 'x86_64-pc-windows-msvc'

    $version = $env:MLXTOP_VERSION
    if (-not $version) {
        $latest = Invoke-RestMethod -Uri 'https://api.github.com/repos/maximpri/mlxtop/releases/latest' `
            -Headers @{ 'User-Agent' = 'mlxtop-installer' }
        $version = $latest.tag_name
    }
    $version = $version -replace '^v', ''
    if ($version -notmatch '^\d+\.\d+\.\d+(-rc\.[1-9]\d*)?$') {
        throw "Could not determine an mlxtop release version (got: $version)."
    }

    $name = "mlxtop-$version-$target"
    $archive = "$name.zip"
    $base = "$repository/releases/download/v$version"
    $installDir = if ($env:MLXTOP_INSTALL_DIR) { $env:MLXTOP_INSTALL_DIR } else {
        Join-Path $env:LOCALAPPDATA 'Programs\mlxtop'
    }
    if (-not [IO.Path]::IsPathRooted($installDir)) {
        throw 'MLXTOP_INSTALL_DIR must be an absolute path.'
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) ("mlxtop-install-" + [Guid]::NewGuid())
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        Write-Host "Downloading mlxtop $version for Windows x86_64..."
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$archive" -OutFile (Join-Path $work $archive)
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile (Join-Path $work 'SHA256SUMS')

        $expected = Get-Content (Join-Path $work 'SHA256SUMS') |
            ForEach-Object { $fields = $_ -split '\s+', 2; if ($fields.Count -eq 2 -and $fields[1].TrimStart('*') -eq $archive) { $fields[0] } } |
            Select-Object -First 1
        if (-not $expected) {
            throw "No checksum published for $archive."
        }
        $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $work $archive)).Hash
        if ($actual -ne $expected.ToUpperInvariant()) {
            throw "Checksum mismatch for ${archive}: expected $expected, got $actual."
        }
        Write-Host "${archive}: OK"

        Expand-Archive -Path (Join-Path $work $archive) -DestinationPath $work
        $payload = Join-Path $work $name
        & (Join-Path $payload 'mlxtop.exe') --version
        if ($LASTEXITCODE -ne 0) {
            throw 'The downloaded mlxtop.exe did not run.'
        }

        New-Item -ItemType Directory -Force -Path $installDir | Out-Null
        foreach ($file in 'mlxtop.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md') {
            Copy-Item -Force (Join-Path $payload $file) $installDir
        }
        $licenses = Join-Path $installDir 'licenses'
        if (Test-Path $licenses) {
            Remove-Item -Recurse -Force $licenses
        }
        Copy-Item -Recurse (Join-Path $payload 'licenses') $licenses
    }
    finally {
        Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Host "Installed $installDir\mlxtop.exe"
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { $_ })
    if ($entries -contains $installDir) {
        Write-Host 'Run: mlxtop'
    }
    elseif ($env:MLXTOP_NO_MODIFY_PATH -eq '1') {
        Write-Host "Add $installDir to PATH to run it as mlxtop."
    }
    else {
        [Environment]::SetEnvironmentVariable('Path', (($entries + $installDir) -join ';'), 'User')
        Write-Host "Added $installDir to your user PATH. Open a new terminal and run: mlxtop"
    }
}

Install-Mlxtop
