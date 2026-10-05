//! Firmware updates over WiFi: the new image is written to the app slot that
//! isn't running, checked, and made the boot slot. The running firmware is
//! never touched, so a failed upload leaves the lamp as it was.

use anyhow::{bail, Context};
use esp_idf_svc::{
    http::server::{EspHttpConnection, Request},
    ota::EspOta,
};

/// One app slot in partitions.csv (`0x1F0000`).
pub const MAX_IMAGE: usize = 0x1F_0000;
/// Read and written at a time.
const CHUNK: usize = 4096;

/// Writes the request body (`len` bytes, a `.bin` from `espflash
/// save-image`) to the free slot and makes it the one to boot. The caller
/// restarts the chip afterwards.
pub fn receive(req: &mut Request<&mut EspHttpConnection<'_>>, len: usize) -> anyhow::Result<()> {
    if len == 0 || len > MAX_IMAGE {
        bail!("{len} bytes: a firmware image must be 1 to {MAX_IMAGE} bytes");
    }
    // Only one instance can exist, so a second upload at the same time fails here.
    let mut ota = EspOta::new().context("another update is running")?;
    // Erases only as much of the slot as the image needs.
    let mut update = ota
        .initiate_update_with_known_size(len)
        .context("start the update")?;
    log::info!("ota: receiving {len} bytes");

    let mut buf = vec![0u8; CHUNK];
    let mut received = 0;
    let mut next_report = len / 4;
    while received < len {
        let n = req.read(&mut buf).context("read the upload")?;
        if n == 0 {
            // `update` is dropped here, which aborts it: the slot stays unused.
            bail!("upload ended after {received} of {len} bytes");
        }
        // The first write also checks that this is an ESP32 image at all.
        update
            .write(&buf[..n])
            .with_context(|| format!("write at byte {received} (not firmware for this chip?)"))?;
        received += n;
        if received >= next_report && received < len {
            log::info!("ota: {}%", received * 100 / len);
            next_report += len / 4;
        }
    }

    // Checks the whole image, then makes its slot the boot slot.
    update.complete().context("the image doesn't check out")?;
    log::info!("ota: done, {received} bytes; restarting into the new firmware");
    Ok(())
}
