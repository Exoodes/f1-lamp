use esp_idf_svc::mdns::EspMdns;

pub const HOSTNAME: &str = "f1-lightbox";

/// Announces `f1-lightbox.local` and the web page. Dropping the returned
/// value stops mDNS, so keep it alive.
pub fn start() -> anyhow::Result<EspMdns> {
    let mut mdns = EspMdns::take()?;
    mdns.set_hostname(HOSTNAME)?;
    mdns.set_instance_name("F1 Lightbox")?;
    mdns.add_service(None, "_http", "_tcp", 80, &[])?;
    log::info!("mDNS: http://{HOSTNAME}.local");
    Ok(mdns)
}
