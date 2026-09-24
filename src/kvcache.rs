// src/kvcache.rs

use crate::config::ModelConfig;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

const KV_DIR: &str = "kv_cache";
const KV_FILENAME: &str = "main.bin";
const KV_META_FILENAME: &str = "main.meta.toml";

/// Parameters that must match for a valid KV cache restore.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct KvMeta {
    model: String,
    ctx_size: u32,
    flash_attn: Option<bool>,
    cache_type_k: String,
    cache_type_v: String,
}

impl KvMeta {
    fn from_config(config: &ModelConfig) -> Self {
        Self {
            model: config.model.clone(),
            ctx_size: config.ctx_size,
            flash_attn: config.flash_attn,
            cache_type_k: "q8_0".to_string(),
            cache_type_v: "q8_0".to_string(),
        }
    }
}

pub struct KvCacheManager {
    port: u16,
    client: reqwest::Client,
}

impl KvCacheManager {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            client: reqwest::Client::new(),
        }
    }

    fn bin_path(&self) -> PathBuf {
        PathBuf::from(KV_DIR).join(KV_FILENAME)
    }

    fn meta_path(&self) -> PathBuf {
        PathBuf::from(KV_DIR).join(KV_META_FILENAME)
    }

    fn slot_url(&self, action: &str) -> String {
        format!("http://127.0.0.1:{}/slots/0?action={}", self.port, action)
    }

    /// Save KV state for slot 0. Writes main.bin via the server API
    /// and KV_META_FILENAME to disk.
    pub async fn save(&self, config: &ModelConfig) -> anyhow::Result<()> {
        let resp = self
            .client
            .post(&self.slot_url("save"))
            .json(&serde_json::json!({ "filename": KV_FILENAME }))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("KV cache save failed ({}): {}", status, body);
        }

        let meta = KvMeta::from_config(config);
        let meta_toml = toml::to_string_pretty(&meta)?;
        std::fs::write(self.meta_path(), meta_toml)?;

        info!("[kvcache] saved to {}", self.bin_path().display());
        Ok(())
    }

    /// Restore KV state for slot 0. Validates fingerprint first.
    /// Returns Ok(true) if restored, Ok(false) if skipped.
    pub async fn restore(&self, config: &ModelConfig) -> anyhow::Result<bool> {
        if !self.bin_path().exists() {
            info!("[kvcache] no cache file, skipping restore");
            return Ok(false);
        }

        if !self.meta_path().exists() {
            warn!("[kvcache] cache exists but no meta file, skipping restore");
            return Ok(false);
        }

        // Validate fingerprint
        let meta_content = std::fs::read_to_string(self.meta_path())?;
        let saved: KvMeta = toml::from_str(&meta_content)?;
        let current = KvMeta::from_config(config);

        if saved != current {
            warn!(
                "[kvcache] fingerprint mismatch, skipping restore. \
                 saved: {:?}, current: {:?}",
                saved, current
            );
            return Ok(false);
        }

        match self
            .client
            .post(&self.slot_url("restore"))
            .json(&serde_json::json!({ "filename": KV_FILENAME }))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                info!("[kvcache] restored from {}", self.bin_path().display());
                Ok(true)
            }
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                warn!("[kvcache] restore failed ({}): {}", status, body);
                Ok(false)
            }
            Err(e) => {
                warn!("[kvcache] restore request failed: {e}");
                Ok(false)
            }
        }
    }
}
