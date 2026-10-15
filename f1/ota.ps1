# Runner for `cargo ota` (see .cargo/config.toml): cargo builds the release
# firmware and calls this with the ELF file. It turns that into an app image,
# uploads it to the lamp over WiFi and waits for the lamp to come back.
#
#   cargo ota                       the normal firmware
#   cargo ota --features live-now   any build works the same way
#
# The lamp is f1-lightbox.local; set $env:F1_LAMP to use another address.

param([Parameter(Mandatory = $true)][string]$Elf)

$ErrorActionPreference = 'Stop'
$lamp = if ($env:F1_LAMP) { $env:F1_LAMP } else { 'f1-lightbox.local' }

# The lamp only takes an upload that proves the key from cfg.toml (build.rs
# checks it: visible ASCII, no quotes). Never printed, never sent.
$keyLine = Select-String -Path cfg.toml -Pattern '^\s*ota_key\s*=\s*"([^"]*)"' | Select-Object -First 1
if (-not $keyLine) {
    Write-Host 'cfg.toml has no ota_key; see the README.'
    exit 1
}
$key = $keyLine.Matches[0].Groups[1].Value
$bin = Join-Path (Split-Path $Elf) 'f1-ota.bin'

# The same layout and slot as a USB flash, so an image too big for a slot
# is refused here rather than by the lamp.
espflash save-image --chip esp32c3 --partition-table partitions.csv `
    --target-app-partition ota_0 $Elf $bin
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# The lamp's IP, looked up while it surely runs. After a restart the name
# can fail for a while: Windows remembers a lookup that failed during the
# reboot and keeps answering "not found". So the upload and the wait for the
# lamp to come back both use the IP, which the router keeps the same.
$ip = curl.exe --silent --max-time 5 -o NUL -w '%{remote_ip}' "http://$lamp/api/state"
$address = if ($LASTEXITCODE -eq 0 -and $ip) { $ip } else { $lamp }

$size = (Get-Item $bin).Length
Write-Host "Uploading $([math]::Round($size / 1MB, 2)) MB to http://$lamp ..."
$started = Get-Date
if ($env:F1_OTA_LEGACY) {
    # Only to update firmware from before the nonce check: it wants the key.
    $auth = @('-H', "X-F1-Key: $key")
} else {
    # A one-time nonce, then HMAC-SHA256(key, nonce + SHA-256(image)) and,
    # checked before the lamp erases anything, HMAC-SHA256(key, nonce +
    # "start" + the size in decimal); see f1_core::ota_auth. Firmware from
    # before the start tag ignores it.
    $nonce = curl.exe --fail-with-body --silent --show-error -X POST `
        -H 'X-F1-Lamp: 1' "http://$address/api/ota/nonce"
    if ($LASTEXITCODE -ne 0) {
        Write-Host ''
        Write-Host 'No nonce from the lamp (see above). If it answered 404, its firmware is'
        Write-Host 'older than the nonce check: send this update once with $env:F1_OTA_LEGACY = 1.'
        exit 1
    }
    $nonceBytes = [byte[]]($nonce.Trim() -split '(..)' -ne '' | ForEach-Object { [Convert]::ToByte($_, 16) })
    $imageHash = [Security.Cryptography.SHA256]::Create().ComputeHash([IO.File]::ReadAllBytes($bin))
    $hmac = New-Object Security.Cryptography.HMACSHA256 (, [Text.Encoding]::UTF8.GetBytes($key))
    $tag = -join ($hmac.ComputeHash([byte[]]($nonceBytes + $imageHash)) | ForEach-Object { $_.ToString('x2') })
    $startBytes = [Text.Encoding]::ASCII.GetBytes("start$size")
    $start = -join ($hmac.ComputeHash([byte[]]($nonceBytes + $startBytes)) | ForEach-Object { $_.ToString('x2') })
    $auth = @('-H', "X-F1-Nonce: $($nonce.Trim())", '-H', "X-F1-Start: $start", '-H', "X-F1-Auth: $tag")
}
# curl.exe, not PowerShell's `curl` alias. --fail-with-body: a refused
# update exits with an error and still shows the lamp's reason.
curl.exe --fail-with-body --silent --show-error `
    --data-binary "@$bin" -H 'Content-Type: application/octet-stream' `
    -H 'X-F1-Lamp: 1' @auth `
    "http://$address/api/ota"
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
    $lastError = curl.exe --silent --show-error --fail --max-time 5 --stderr - -o NUL "http://$address/api/state"
    if ($LASTEXITCODE -eq 0) {
        Write-Host "The lamp is back: http://$lamp ($address)"
        exit 0
    }
    Start-Sleep -Seconds 1
}
Write-Host "The lamp hasn't answered for 60 s; check it (or its log over USB)."
Write-Host "Last attempt: $lastError"
exit 1
