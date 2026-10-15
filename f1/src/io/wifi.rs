//! WiFi in station mode: set up once, connect again whenever it's down.

use std::time::Duration;

use anyhow::Context;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::modem::Modem,
    nvs::EspDefaultNvsPartition,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};

/// The WiFi driver in station mode, with blocking connect calls.
pub type Wifi = BlockingWifi<EspWifi<'static>>;

/// The longest the lamp waits for the router to take it; it normally does
/// in about 3 s (a scan, then the association). A rejected association
/// never connects, so esp-idf's own 15 s only delayed the retry, which then
/// worked (after every USB flash, 2026-10-15).
const CONNECT_WAIT: Duration = Duration::from_secs(8);

/// Sets up and starts WiFi in station mode, without connecting yet.
/// Call once: it takes the modem. The connection lives as long as the
/// returned value, so drop it and WiFi stops.
pub fn create(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
    password: &str,
) -> anyhow::Result<Wifi> {
    let driver = EspWifi::new(modem, sys_loop.clone(), Some(nvs)).context("create wifi driver")?;
    let mut wifi = BlockingWifi::wrap(driver, sys_loop).context("wrap wifi driver")?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID is longer than 32 bytes"))?,
        password: password
            .try_into()
            .map_err(|_| anyhow::anyhow!("password is longer than 64 bytes"))?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))
    .context("configure wifi")?;

    wifi.start().context("start wifi")?;
    log::info!("wifi started, SSID {ssid}");
    Ok(wifi)
}

/// Connects and blocks until the router has given us an IP address.
/// Safe to call again after a failure or a lost connection.
pub fn connect(wifi: &mut Wifi) -> anyhow::Result<()> {
    // A retry may find the driver half-connected; start from a clean state.
    // Fails harmlessly when there is nothing to disconnect.
    let _ = wifi.disconnect();
    wifi.wifi_mut().connect().context("connect to wifi")?;
    wifi.wifi_wait_while(
        || wifi.wifi().is_connected().map(|connected| !connected),
        Some(CONNECT_WAIT),
    )
    .context("connect to wifi")?;
    wifi.wait_netif_up().context("wait for IP address")?;

    let ip_info = wifi
        .wifi()
        .sta_netif()
        .get_ip_info()
        .context("read IP address")?;
    log::info!("wifi up, IP {}", ip_info.ip);

    Ok(())
}
