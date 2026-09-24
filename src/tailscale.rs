// src/tailscale.rs

use serde::Deserialize;
use std::net::Ipv4Addr;
use tracing::debug;

pub struct TailscaleInfo {
    pub ip: Ipv4Addr,
    pub dns_name: Option<String>,
}

#[derive(Deserialize)]
struct StatusJson {
    #[serde(rename = "BackendState")]
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: SelfNode,
}

#[derive(Deserialize)]
struct SelfNode {
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Vec<String>,
    #[serde(rename = "DNSName")]
    dns_name: Option<String>,
}

/// Check Tailscale status.
///
/// - `Ok(Some(info))` — Tailscale is running, URLs available.
/// - `Ok(None)` — `tailscale` binary not found, silent skip.
/// - `Err(message)` — Tailscale is installed but not usable, caller should warn.
pub async fn status() -> Result<Option<TailscaleInfo>, String> {
    let output = match tokio::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            debug!("tailscale binary not found, skipping");
            return Ok(None);
        }
        Err(e) => return Err(format!("Failed to run tailscale: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("is tailscaled running") || stderr.contains("not running") {
            return Err(
                "Tailscale daemon not running. Start with: sudo systemctl start tailscaled".into(),
            );
        }
        return Err(format!("tailscale status failed: {}", stderr.trim()));
    }

    let parsed: StatusJson = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse tailscale status: {e}"))?;

    match parsed.backend_state.as_str() {
        "Running" => {}
        "NeedsLogin" => return Err("Tailscale not authenticated. Run: tailscale up".into()),
        "Stopped" => return Err("Tailscale is stopped. Run: tailscale up".into()),
        other => return Err(format!("Tailscale state: {other}. Run: tailscale up")),
    }

    let ip_str = parsed
        .self_node
        .tailscale_ips
        .iter()
        .find(|s| s.parse::<Ipv4Addr>().is_ok())
        .ok_or_else(|| "No IPv4 Tailscale IP found".to_string())?;
    let ip: Ipv4Addr = ip_str
        .parse()
        .map_err(|e| format!("Failed to parse Tailscale IP: {e}"))?;

    // DNS name comes through with a trailing dot, e.g. "framework.tail-abc.ts.net."
    let dns_name = parsed
        .self_node
        .dns_name
        .map(|s| s.trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty());

    Ok(Some(TailscaleInfo { ip, dns_name }))
}
