use anyhow::Context;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::modem::Modem,
    nvs::EspDefaultNvsPartition,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};

pub fn connect(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
    password: &str,
) -> anyhow::Result<BlockingWifi<EspWifi<'static>>> {
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
    log::info!("connecting to {ssid}");
    wifi.connect().context("connect to wifi")?;
    wifi.wait_netif_up().context("wait for IP address")?;

    let ip_info = wifi
        .wifi()
        .sta_netif()
        .get_ip_info()
        .context("read IP address")?;
    log::info!("wifi up, IP {}", ip_info.ip);

    Ok(wifi)
}
