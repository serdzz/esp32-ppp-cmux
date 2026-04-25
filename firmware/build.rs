use std::env;
use std::fs;
use std::path::PathBuf;

/// Pulls cellular + MQTT settings from env (or `.env`) and re-exports them
/// as `rustc-env`.
///
/// Missing required vars emit `cargo:warning` rather than panicking — first
/// builds (e.g. CI smoke tests) succeed with empty placeholders; runtime
/// will fail at the relevant init step instead.
fn main() {
    let firmware_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = firmware_dir.parent().unwrap();
    for candidate in [firmware_dir.join(".env"), workspace_root.join(".env")] {
        if candidate.exists() {
            let _ = dotenvy::from_path(&candidate);
            println!("cargo:rerun-if-changed={}", candidate.display());
            break;
        }
    }

    let required = ["APN", "MQTT_HOST", "MQTT_PORT", "MQTT_CLIENT_ID"];
    let optional = [
        "GPRS_USER", "GPRS_PASS", "MQTT_USER", "MQTT_PASS", "SIM_PIN", "MQTT_DNS",
    ];

    for k in required {
        let v = env::var(k).unwrap_or_else(|_| {
            println!("cargo:warning=env var {k} is empty — runtime init will fail");
            String::new()
        });
        forward(k, &v);
    }
    for k in optional {
        forward(k, &env::var(k).unwrap_or_default());
    }
    forward("MQTT_PORT_FALLBACK", "8883");

    // CA cert: copy into OUT_DIR (or write empty stub) so include_bytes!
    // always sees a valid path.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ca_target = out_dir.join("mqtt_ca.pem");
    let ca_src = env::var("MQTT_CA_PEM").unwrap_or_default();
    let ca_path = if ca_src.is_empty() {
        PathBuf::new()
    } else {
        firmware_dir.join(&ca_src)
    };
    if ca_path.exists() {
        fs::copy(&ca_path, &ca_target).expect("copy CA cert");
        println!("cargo:rerun-if-changed={}", ca_path.display());
    } else {
        fs::write(&ca_target, b"").expect("write empty CA stub");
        if !ca_src.is_empty() {
            println!(
                "cargo:warning=MQTT_CA_PEM file '{ca_src}' not found — using empty stub, TLS handshake will fail"
            );
        }
    }
    println!("cargo:rustc-env=MQTT_CA_PEM_PATH={}", ca_target.display());

    for k in required.iter().chain(optional.iter()).chain(&["MQTT_CA_PEM"]) {
        println!("cargo:rerun-if-env-changed={k}");
    }
}

fn forward(name: &str, value: &str) {
    let trimmed = value.trim().trim_matches('"').trim_matches('\'');
    println!("cargo:rustc-env={name}={trimmed}");
}
