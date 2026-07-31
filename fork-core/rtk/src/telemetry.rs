use crate::config;
use crate::tracking;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const TELEMETRY_URL: Option<&str> = option_env!("RTK_TELEMETRY_URL");
const TELEMETRY_TOKEN: Option<&str> = option_env!("RTK_TELEMETRY_TOKEN");
const PING_INTERVAL_SECS: u64 = 23 * 3600; // 23 hours

/// Send a telemetry ping if enabled and not already sent today.
/// Fire-and-forget: errors are silently ignored.
pub fn maybe_ping() {
    // No URL compiled in → telemetry disabled
    if TELEMETRY_URL.is_none() {
        return;
    }

    // Check opt-out: env var
    if std::env::var("RTK_TELEMETRY_DISABLED").unwrap_or_default() == "1" {
        return;
    }

    // Require explicit consent before any telemetry thread or network request.
    match config::telemetry_consent() {
        Some(true) => {}
        Some(false) | None => return,
    }

    // Check opt-out: config.toml
    if let Some(false) = config::telemetry_enabled() {
        return;
    }

    // Check last ping time
    let marker = telemetry_marker_path();
    if let Ok(metadata) = std::fs::metadata(&marker) {
        if let Ok(modified) = metadata.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed.as_secs() < PING_INTERVAL_SECS {
                    return;
                }
            }
        }
    }

    // Touch marker file immediately (before sending) to avoid double-ping
    touch_marker(&marker);

    // Spawn thread so we never block the CLI
    std::thread::spawn(|| {
        let _ = send_ping();
    });
}

fn send_ping() -> Result<(), Box<dyn std::error::Error>> {
    let url = TELEMETRY_URL.ok_or("no telemetry URL")?;
    let device_hash = generate_device_hash();
    let version = env!("CARGO_PKG_VERSION").to_string();
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();

    // Get stats from tracking DB
    let (commands_24h, top_commands, savings_pct) = get_stats();

    let payload = serde_json::json!({
        "device_hash": device_hash,
        "version": version,
        "os": os,
        "arch": arch,
        "commands_24h": commands_24h,
        "top_commands": top_commands,
        "savings_pct": savings_pct,
    });

    let mut req = ureq::post(url).set("Content-Type", "application/json");

    if let Some(token) = TELEMETRY_TOKEN {
        req = req.set("X-RTK-Token", token);
    }

    // 2 second timeout — if server is down, we move on
    req.timeout(std::time::Duration::from_secs(2))
        .send_string(&payload.to_string())?;

    Ok(())
}

pub fn generate_device_hash() -> String {
    let salt = get_or_create_salt();
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn get_or_create_salt() -> String {
    let path = salt_file_path();

    if let Ok(salt) = std::fs::read_to_string(&path) {
        let trimmed = salt.trim();
        if trimmed.len() >= 32 {
            return trimmed.to_string();
        }
    }

    let salt = random_salt();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _ = std::fs::write(&path, &salt);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    salt
}

fn random_salt() -> String {
    let mut buf = [0u8; 32];
    if getrandom::getrandom(&mut buf).is_err() {
        let fallback = format!(
            "{}:{}:{:?}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            std::time::SystemTime::now()
        );
        let mut hasher = Sha256::new();
        hasher.update(fallback.as_bytes());
        let digest = hasher.finalize();
        return digest.iter().map(|b| format!("{:02x}", b)).collect();
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn salt_file_path() -> PathBuf {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("rtk");
    data_dir.join(".device_salt")
}

fn get_stats() -> (i64, Vec<String>, Option<f64>) {
    let tracker = match tracking::Tracker::new() {
        Ok(t) => t,
        Err(_) => return (0, vec![], None),
    };

    // Get 24h command count and top commands from tracking DB
    let commands_24h = tracker
        .count_commands_since(chrono::Utc::now() - chrono::Duration::hours(24))
        .unwrap_or(0);

    let top_commands = tracker.top_commands(5).unwrap_or_default();

    let savings_pct = tracker.overall_savings_pct().ok();

    (commands_24h, top_commands, savings_pct)
}

pub fn telemetry_marker_path() -> PathBuf {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("rtk");
    let _ = std::fs::create_dir_all(&data_dir);
    data_dir.join(".telemetry_last_ping")
}

fn touch_marker(path: &PathBuf) {
    let _ = std::fs::write(path, b"");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_hash_is_stable() {
        let h1 = generate_device_hash();
        let h2 = generate_device_hash();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn test_marker_path_exists() {
        let path = telemetry_marker_path();
        assert!(path.to_string_lossy().contains("rtk"));
    }

    #[test]
    fn test_salt_path_is_in_rtk_data_dir() {
        let path = salt_file_path();
        assert!(path.to_string_lossy().contains("rtk"));
        assert!(path.to_string_lossy().contains(".device_salt"));
    }
}
