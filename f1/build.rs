//! Hands `cfg.toml`'s WiFi credentials and update key to the compiler.

fn main() {
    embuild::espidf::sysenv::output();
    println!("cargo:rerun-if-changed=cfg.toml");
    let cfg = read_cfg();
    let f1 = cfg
        .get("f1")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("f1/cfg.toml has no [f1] table"));
    wifi_credentials(f1);
    ota_key(f1);
}

/// Reads `cfg.toml`.
///
/// Read here rather than with the `toml-cfg` crate, which finds the file
/// through rustc's `--out-dir` argument: on Windows our rustc command line is
/// too long, Cargo passes the arguments in a file instead, and `toml-cfg`
/// silently used its defaults (an empty SSID).
fn read_cfg() -> toml::Table {
    // Error messages never quote the file's contents: they could show the password.
    let text = std::fs::read_to_string("cfg.toml").unwrap_or_else(|_| {
        panic!(
            "can't read f1/cfg.toml: create it with an [f1] table holding wifi_ssid, wifi_psk and ota_key"
        )
    });
    text.parse()
        .unwrap_or_else(|_| panic!("f1/cfg.toml is not valid TOML"))
}

/// Hands the WiFi credentials to the compiler as `WIFI_SSID` and `WIFI_PSK`,
/// which `config.rs` bakes in with `env!`.
fn wifi_credentials(f1: &toml::Table) {
    for (key, var) in [("wifi_ssid", "WIFI_SSID"), ("wifi_psk", "WIFI_PSK")] {
        let value = f1
            .get(key)
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| panic!("f1/cfg.toml: [f1] has no text value `{key}`"));
        println!("cargo:rustc-env={var}={value}");
    }
}

/// The shortest `ota_key` accepted: anything shorter is guessable.
const MIN_OTA_KEY: usize = 16;

/// Hands the update key to the compiler as `OTA_KEY`. The lamp only takes a
/// firmware upload whose tag proves it (`f1_core::ota_auth`); `ota.ps1`
/// reads it from cfg.toml too.
fn ota_key(f1: &toml::Table) {
    let key = f1
        .get("ota_key")
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "f1/cfg.toml: [f1] has no `ota_key`: add a random text of at least \
                 {MIN_OTA_KEY} characters, e.g. ota_key = \"<32 random letters>\""
            )
        });
    // Checked here so a bad key stops the build, not an update later: ota.ps1
    // reads it between quotes.
    if key.len() < MIN_OTA_KEY {
        panic!("f1/cfg.toml: ota_key must be at least {MIN_OTA_KEY} characters");
    }
    if !key.chars().all(|c| c.is_ascii_graphic() && c != '"') {
        panic!("f1/cfg.toml: ota_key may only hold visible ASCII characters, no quotes");
    }
    println!("cargo:rustc-env=OTA_KEY={key}");
}
