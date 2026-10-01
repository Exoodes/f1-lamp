fn main() {
    embuild::espidf::sysenv::output();
    wifi_credentials();
}

/// Reads the WiFi credentials from `cfg.toml` and hands them to the compiler as
/// `WIFI_SSID` and `WIFI_PSK`, which `main.rs` bakes in with `env!`.
///
/// This replaces `toml-cfg`: it finds `cfg.toml` through rustc's `--out-dir`
/// argument, but on Windows our rustc command line is too long, Cargo passes
/// the arguments in a file instead, and `toml-cfg` silently used its defaults.
fn wifi_credentials() {
    println!("cargo:rerun-if-changed=cfg.toml");

    // Error messages never quote the file's contents: they could show the password.
    let text = std::fs::read_to_string("cfg.toml").unwrap_or_else(|_| {
        panic!(
            "can't read f1/cfg.toml: create it with an [f1] table holding wifi_ssid and wifi_psk"
        )
    });
    let cfg: toml::Table = text
        .parse()
        .unwrap_or_else(|_| panic!("f1/cfg.toml is not valid TOML"));
    let f1 = cfg
        .get("f1")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("f1/cfg.toml has no [f1] table"));

    for (key, var) in [("wifi_ssid", "WIFI_SSID"), ("wifi_psk", "WIFI_PSK")] {
        let value = f1
            .get(key)
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| panic!("f1/cfg.toml: [f1] has no text value `{key}`"));
        println!("cargo:rustc-env={var}={value}");
    }
}
