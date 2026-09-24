// src/serve.rs

use crate::config::ModelConfig;

use std::fs::File;
use std::process::Stdio;
use tokio::process::Command;
use tracing::info;

pub struct ServeHandle {
    child: tokio::process::Child,
    port: u16,
}

impl ServeHandle {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn start(config: &ModelConfig, log_path: &str) -> anyhow::Result<Self> {
        // Allocate a random free port for llama-server.
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = probe.local_addr()?.port();
        drop(probe);

        let log_file = File::create(log_path)?;
        let log_stderr = log_file.try_clone()?;

        if config.kv_cache {
            std::fs::create_dir_all("kv_cache")?;
        }

        let mut cmd = Command::new("llama-server");

        // Model
        cmd.arg("--model").arg(&config.model);

        // Server
        cmd.arg("--port").arg(port.to_string());
        cmd.arg("--no-webui");
        cmd.arg("--offline");

        // Hardware
        cmd.arg("--threads").arg(config.threads.to_string());
        cmd.arg("--gpu-layers").arg(config.ngl.to_string());
        if config.device != "none" {
            cmd.arg("--device").arg(&config.device);
        }

        // Context and batching
        cmd.arg("--ctx-size").arg(config.ctx_size.to_string());
        cmd.arg("--parallel").arg(config.parallel.to_string());
        cmd.arg("--batch-size").arg(config.batch_size.to_string());
        cmd.arg("--cache-ram").arg(config.cache_ram.to_string());
        cmd.arg("--cache-reuse").arg(config.cache_reuse.to_string());

        // Flash attention
        match config.flash_attn {
            Some(true) => {
                cmd.arg("--flash-attn").arg("on");
            }
            Some(false) => {
                cmd.arg("--flash-attn").arg("off");
            }
            None => {}
        }

        // Memory mapping
        if config.no_mmap {
            cmd.arg("--no-mmap");
        }

        // KV cache persistence
        if config.kv_cache {
            let abs_path = std::path::Path::new("kv_cache").canonicalize()?;
            cmd.arg("--slot-save-path").arg(abs_path);
            cmd.arg("--cache-type-k").arg("q8_0");
            cmd.arg("--cache-type-v").arg("q8_0");
        }

        // Best-effort: ask the kernel to kill llama-server if weftd dies without
        // running Drop (SIGKILL, abort, panic = abort). Linux only — macOS has no
        // equivalent, so there we rely on Drop plus startup reaping.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        unsafe {
            cmd.pre_exec(|| {
                // Safety: async-signal-safe, no allocation between fork and exec.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        cmd.stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_stderr));

        #[cfg(unix)]
        cmd.process_group(0);

        let child = cmd.spawn()?;

        info!("llama-server output logged to {log_path}");
        info!("llama-server started on port {port}");

        Ok(Self { child, port })
    }

    // pub async fn stop(&mut self) -> anyhow::Result<()> {
    //     self.child.kill().await?;
    //     Ok(())
    // }

    pub async fn wait_until_ready(&mut self, port: u16, timeout_secs: u64) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        let addr = format!("127.0.0.1:{port}");
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/health");

        while tokio::time::Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                anyhow::bail!(
                    "llama-server exited during startup with {status}. Check the serve log."
                );
            }

            let ok = client
                .get(&url)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if ok {
                info!("llama-server is ready on port {port}");
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        anyhow::bail!("llama-server did not become ready within {timeout_secs}s")
    }
}

impl Drop for ServeHandle {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
