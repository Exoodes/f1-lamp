use std::path::{Path, PathBuf};

/// The streams a replay embeds, with the variable that hands each file's path
/// to `src/replay.rs`. Required ones stop the build when missing; without an
/// optional one, its events (winner, pit stops, ...) just don't show.
const REPLAY_FILES: [(&str, &str, bool); 9] = [
    ("SessionInfo", "REPLAY_SESSION_INFO", false),
    ("TrackStatus", "REPLAY_TRACK_STATUS", true),
    ("SessionStatus", "REPLAY_SESSION_STATUS", true),
    ("RaceControlMessages", "REPLAY_RACE_CONTROL", true),
    ("DriverList", "REPLAY_DRIVER_LIST", false),
    ("TopThree", "REPLAY_TOP_THREE", false),
    ("PitLaneTimeCollection", "REPLAY_PIT_LANE", false),
    ("TimingStats", "REPLAY_TIMING_STATS", false),
    ("OvertakeSeries", "REPLAY_OVERTAKES", false),
];

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
    replay_dir(cfg.get("replay").and_then(toml::Value::as_table));
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
/// which `main.rs` bakes in with `env!`.
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
/// firmware upload that carries it (`web.rs`); `ota.ps1` reads it from
/// cfg.toml too.
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
    // Checked here so a bad key stops the build, not an update later. Header
    // values can't hold control characters, and ota.ps1 reads it between quotes.
    if key.len() < MIN_OTA_KEY {
        panic!("f1/cfg.toml: ota_key must be at least {MIN_OTA_KEY} characters");
    }
    if !key.chars().all(|c| c.is_ascii_graphic() && c != '"') {
        panic!("f1/cfg.toml: ota_key may only hold visible ASCII characters, no quotes");
    }
    println!("cargo:rustc-env=OTA_KEY={key}");
}

/// With `--features replay` or `showcase`: finds the files of the session to
/// play and hands their paths to the compiler (see `REPLAY_FILES`), which
/// `src/replay.rs` embeds with `include_str!`.
///
/// `replay` takes the folder from `$F1_REPLAY_DIR`, else `[replay] dir` in
/// cfg.toml, so another session can be tried without editing the file.
/// `showcase` always plays the hand-written race in f1-core's test data.
fn replay_dir(replay: Option<&toml::Table>) {
    println!("cargo:rerun-if-env-changed=F1_REPLAY_DIR");
    let wants_replay = std::env::var_os("CARGO_FEATURE_REPLAY").is_some();
    let wants_showcase = std::env::var_os("CARGO_FEATURE_SHOWCASE").is_some();
    let dir = match (wants_replay, wants_showcase) {
        (false, false) => return,
        (true, true) => panic!("--features replay and showcase both play a session: pick one"),
        (false, true) => "../f1-core/tests/data/showcase".to_owned(),
        (true, false) => std::env::var("F1_REPLAY_DIR")
            .ok()
            .or_else(|| {
                replay
                    .and_then(|t| t.get("dir"))
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| {
                panic!(
                    "--features replay needs a session: add\n\n[replay]\n\
                     dir = \"C:/path/to/f1-data/<weekend>/<session>\"\n\n\
                     to f1/cfg.toml, or set F1_REPLAY_DIR"
                )
            }),
    };
    // A relative path is taken from f1/.
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let dir = Path::new(&manifest).join(dir);
    if !dir.is_dir() {
        panic!("replay: {} is not a folder", slashes(&dir));
    }
    println!("cargo:rustc-env=REPLAY_DIR={}", slashes(&dir));
    // Watch the folders too: a file that appears later (a stream downloaded
    // or written afterwards) must make this run again.
    println!("cargo:rerun-if-changed={}", slashes(&dir));
    for sub in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        if sub.path().is_dir() {
            println!("cargo:rerun-if-changed={}", slashes(&sub.path()));
        }
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    for (stream, var, required) in REPLAY_FILES {
        let name = format!("{stream}.jsonStream");
        let path = match find(&dir, &name) {
            Some(path) => {
                println!("cargo:rerun-if-changed={}", slashes(&path));
                trimmed(&path, stream, &out_dir.join(&name))
            }
            None if required => panic!("replay: no {name} in {} or its subfolders", slashes(&dir)),
            None => {
                println!(
                    "cargo:warning=replay: no {name} in {}; its events won't show",
                    slashes(&dir)
                );
                // An empty stream: the replay plays without it.
                let empty = out_dir.join(&name);
                std::fs::write(&empty, "").unwrap();
                empty
            }
        };
        println!("cargo:rustc-env={var}={}", slashes(&path));
    }
}

/// Writes the lines of `source` the replay can use to `target` and returns
/// `target`. The rest would be skipped unparsed anyway (`Stream::is_relevant`),
/// so leaving them out changes nothing but the size: TopThree and
/// TimingStats are about 200 KB each for a race, and with them whole a
/// replay build doesn't fit an app slot. Lines that don't even split are kept,
/// so the replay still reports them.
fn trimmed(source: &Path, stream: &str, target: &Path) -> PathBuf {
    use f1_core::{stream::parse_line, timeline::Stream};

    let text = std::fs::read_to_string(source)
        .unwrap_or_else(|e| panic!("replay: can't read {}: {e}", slashes(source)));
    let Some(kind) = Stream::from_name(stream) else {
        panic!("replay: {stream} is not a stream the replay knows");
    };
    let kept: Vec<&str> = text
        .lines()
        .filter(|raw| match parse_line(raw) {
            Ok(line) => kind.is_relevant(line.json),
            Err(_) => !raw.trim().is_empty(),
        })
        .collect();
    std::fs::write(target, kept.join("\n"))
        .unwrap_or_else(|e| panic!("replay: can't write {}: {e}", slashes(target)));
    target.to_path_buf()
}

/// `name` in `dir` or in one of its direct subfolders, which is where the
/// download script sorts streams (`1-lamp-core/`, `2-lamp-maybe/`, ...).
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    subdirs.sort();
    subdirs
        .into_iter()
        .map(|sub| sub.join(name))
        .find(|path| path.is_file())
}

/// Forward slashes work on Windows too and avoid escaping trouble inside
/// `concat!` and `include_str!`.
fn slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
