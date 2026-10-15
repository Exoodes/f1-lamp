# Every check the CI runs (.github/workflows/ci.yml), the same way on this
# PC: the web page's script syntax (needs Node.js), formatting, clippy with
# warnings as errors, the f1-core tests, the probe, and the firmware's release
# build with the image-size check. Any
# warning in a check's output fails it. Only the security audit of the lock
# files runs in CI alone.
#
#   powershell -ExecutionPolicy Bypass -File check.ps1             everything
#   powershell -ExecutionPolicy Bypass -File check.ps1 -Part core  no firmware
#
# The firmware part needs f1/cfg.toml (CI writes a dummy one), ldproxy and
# espflash, like any firmware build. Exits with 1 if anything failed.

param([ValidateSet('all', 'core', 'firmware')][string]$Part = 'all')

$ErrorActionPreference = 'Continue'
$root = $PSScriptRoot
$failed = New-Object System.Collections.Generic.List[string]

# An image above this share of an app slot fails: an update over WiFi needs
# the image to fit.
$maxSlotPercent = 95

# One line of a native command's output: a stderr line arrives as an error
# record, whose own text would be its type name when the line is empty.
function Get-Text($line) {
    if ($line -is [System.Management.Automation.ErrorRecord]) {
        return $line.Exception.Message
    }
    return "$line"
}

# Runs `cargo <CargoArgs>` in `Dir`. Shows the output only when it fails.
function Invoke-Cargo {
    param([string]$Name, [string]$Dir, [string[]]$CargoArgs)
    Write-Host "== $Name" -ForegroundColor Cyan
    Push-Location (Join-Path $root $Dir)
    # Each line as plain text: Windows PowerShell wraps a native command's
    # stderr lines in error records, which print with noise around them.
    $output = & cargo @CargoArgs 2>&1 | ForEach-Object { Get-Text $_ } | Out-String
    $code = $LASTEXITCODE
    Pop-Location
    # Warnings that clippy's -D warnings doesn't reach (e.g. an unused
    # dependency in Cargo.toml) fail the check too.
    if ($code -eq 0 -and $output -match '(?m)^warning:') {
        $code = 1
    }
    if ($code -ne 0) {
        Write-Host $output
        Write-Host "FAILED: $Name" -ForegroundColor Red
        $script:failed.Add($Name)
        return $false
    }
    return $true
}

# Turns the release build just made into an app image, as an update would,
# and checks it fits the app slot with room to spare.
function Test-ImageSize {
    param([string]$Name)
    $elf = Join-Path $root 'f1/target/riscv32imc-esp-espidf/release/f1'
    $bin = Join-Path ([IO.Path]::GetTempPath()) 'f1-check.bin'
    Push-Location (Join-Path $root 'f1')
    $output = & espflash save-image --chip esp32c3 --partition-table partitions.csv `
        --target-app-partition ota_0 $elf $bin 2>&1 | ForEach-Object { Get-Text $_ } | Out-String
    $code = $LASTEXITCODE
    Pop-Location
    if ($code -ne 0 -or $output -notmatch 'App/part\. size:\s+([\d,]+)/([\d,]+) bytes, ([\d.]+)%') {
        Write-Host $output
        Write-Host "FAILED: $Name image" -ForegroundColor Red
        $script:failed.Add("$Name image")
        return
    }
    # Invariant culture: a Czech Windows would read "87.51" as 8751.
    $percent = [double]::Parse($Matches[3], [Globalization.CultureInfo]::InvariantCulture)
    if ($percent -gt $maxSlotPercent) {
        Write-Host "FAILED: $Name image is $percent % of the app slot (limit $maxSlotPercent %)" -ForegroundColor Red
        $script:failed.Add("$Name image")
    } else {
        Write-Host "   image: $($Matches[1]) of $($Matches[2]) bytes, $percent %"
    }
}

# Checks the syntax of the web page's script: one error stops the whole page
# (race day 2026-10-11: a line break inside a string, "connecting…" forever),
# and the firmware builds don't look inside it.
function Test-PageScript {
    $name = 'web: page script'
    Write-Host "== $name" -ForegroundColor Cyan
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
        Write-Host 'Node.js is needed: winget install OpenJS.NodeJS.LTS' -ForegroundColor Red
        Write-Host "FAILED: $name" -ForegroundColor Red
        $script:failed.Add($name)
        return
    }
    $page = Get-Content -Raw -Encoding UTF8 (Join-Path $root 'f1/web/index.html')
    $scripts = [regex]::Matches($page, '(?s)<script>(.*?)</script>')
    if ($scripts.Count -eq 0) {
        Write-Host 'no <script> found in f1/web/index.html' -ForegroundColor Red
        Write-Host "FAILED: $name" -ForegroundColor Red
        $script:failed.Add($name)
        return
    }
    $js = Join-Path ([IO.Path]::GetTempPath()) 'f1-check-page.js'
    foreach ($s in $scripts) {
        # Blank lines in place of the HTML before it, so an error's line
        # number is the line in index.html.
        $before = ([regex]::Matches($page.Substring(0, $s.Groups[1].Index), "`n")).Count
        [IO.File]::WriteAllText($js, ("`n" * $before) + $s.Groups[1].Value)
        $output = & node --check $js 2>&1 | ForEach-Object { Get-Text $_ } | Out-String
        if ($LASTEXITCODE -ne 0) {
            Write-Host $output
            Write-Host "FAILED: $name" -ForegroundColor Red
            $script:failed.Add($name)
            return
        }
    }
}

if ($Part -ne 'firmware') {
    Test-PageScript
    Invoke-Cargo 'f1-core: format' 'f1-core' @('fmt', '--check') | Out-Null
    Invoke-Cargo 'f1-core: clippy' 'f1-core' @('clippy', '--all-targets', '--', '-D', 'warnings') | Out-Null
    Invoke-Cargo 'f1-core: tests' 'f1-core' @('test') | Out-Null
    Invoke-Cargo 'f1-probe: format' 'f1-probe' @('fmt', '--check') | Out-Null
    Invoke-Cargo 'f1-probe: clippy' 'f1-probe' @('clippy', '--all-targets', '--', '-D', 'warnings') | Out-Null
}

if ($Part -ne 'core') {
    if (-not (Test-Path (Join-Path $root 'f1/cfg.toml'))) {
        Write-Host 'FAILED: f1/cfg.toml is missing (see the README)' -ForegroundColor Red
        $failed.Add('f1/cfg.toml')
    } else {
        Invoke-Cargo 'f1: format' 'f1' @('fmt', '--all', '--check') | Out-Null
        Invoke-Cargo 'f1: clippy' 'f1' @('clippy', '--release', '--', '-D', 'warnings') | Out-Null
        if (Invoke-Cargo 'f1: build' 'f1' @('build', '--release')) {
            Test-ImageSize 'f1'
        }
    }
}

Write-Host ''
if ($failed.Count -eq 0) {
    Write-Host "All checks passed ($Part)." -ForegroundColor Green
    exit 0
}
Write-Host "Failed: $($failed -join ', ')" -ForegroundColor Red
exit 1
