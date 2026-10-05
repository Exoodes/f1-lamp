# Runner for `cargo ota` (see .cargo/config.toml): cargo builds the release
# firmware and calls this with the ELF file. It turns that into an app image,
# uploads it to the lamp over WiFi and waits for the lamp to come back.
#
#   cargo ota                       the normal firmware
#   cargo ota --features showcase   any build works the same way
#
# The lamp is f1-lightbox.local; set $env:F1_LAMP to use another address.

param([Parameter(Mandatory = $true)][string]$Elf)

$ErrorActionPreference = 'Stop'
$lamp = if ($env:F1_LAMP) { $env:F1_LAMP } else { 'f1-lightbox.local' }
$bin = Join-Path (Split-Path $Elf) 'f1-ota.bin'

# The same layout and slot as a USB flash, so an image too big for a slot
# is refused here rather than by the lamp.
espflash save-image --chip esp32c3 --partition-table partitions.csv `
    --target-app-partition ota_0 $Elf $bin
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# The lamp's IP, looked up while it surely runs. After its restart the name
# can fail for a while: Windows remembers a lookup that failed during the
# reboot and keeps answering "not found". The router gives the lamp the same
# IP again, so waiting for the IP avoids the name entirely.
$ip = curl.exe --silent --max-time 5 -o NUL -w '%{remote_ip}' "http://$lamp/api/state"
$back = if ($LASTEXITCODE -eq 0 -and $ip) { $ip } else { $lamp }

$size = (Get-Item $bin).Length
Write-Host "Uploading $([math]::Round($size / 1MB, 2)) MB to http://$lamp ..."
$started = Get-Date
# curl.exe, not PowerShell's `curl` alias. --fail-with-body: a refused
# update exits with an error and still shows the lamp's reason.
curl.exe --fail-with-body --silent --show-error `
    --data-binary "@$bin" -H 'Content-Type: application/octet-stream' `
    "http://$lamp/api/ota"
if ($LASTEXITCODE -ne 0) {
    Write-Host ''
    Write-Host 'Update failed; the lamp keeps running its current firmware.'
    exit 1
}
Write-Host ''
Write-Host "Uploaded in $([math]::Round(((Get-Date) - $started).TotalSeconds)) s, the lamp is restarting."

# Back once the web server answers again (a restart takes ~5 s).
$deadline = (Get-Date).AddSeconds(60)
Start-Sleep -Seconds 3
$lastError = ''
while ((Get-Date) -lt $deadline) {
    # curl.exe here too: PowerShell's Invoke-RestMethod needs about 3 s just
    # to resolve a .local name, so with a short timeout it never got through.
    # --stderr -: errors on stdout. A `2>&1` would turn them into PowerShell
    # errors, which `$ErrorActionPreference = 'Stop'` makes fatal.
    $lastError = curl.exe --silent --show-error --fail --max-time 5 --stderr - -o NUL "http://$back/api/state"
    if ($LASTEXITCODE -eq 0) {
        Write-Host "The lamp is back: http://$lamp ($back)"
        exit 0
    }
    Start-Sleep -Seconds 1
}
Write-Host "The lamp hasn't answered for 60 s; check it (or its log over USB)."
Write-Host "Last attempt: $lastError"
exit 1
