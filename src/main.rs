// src/main.rs

mod chat;
mod cli;
mod client;
mod command;
mod config;
mod connection;
mod execute;
mod kvcache;
mod prompt;
mod render;
mod retrieval;
mod serve;
mod session;
mod summary;
mod tailscale;
mod utils;
mod whisper;

use crate::chat::{Conversation, SharedConversation, SummaryState};
use crate::config::ModelConfig;
use crate::retrieval::{NamedRetrieval, RetrievalEngine};
use crate::whisper::WhisperStack;

use axum::Router;
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::response::Html;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() {
    let args = cli::parse();

    let path = args.path.unwrap_or_else(|| {
        eprintln!("No TOML file specified. Usage: weftd <path/to/config.toml>");
        std::process::exit(1);
    });

    let config = ModelConfig::load(std::path::Path::new(&path)).unwrap_or_else(|e| {
        eprintln!("Failed to load {path}: {e}");
        std::process::exit(1);
    });
    config.summary_config.validate();

    init_tracing(config.log_metrics);

    let mut serve = serve::ServeHandle::start(&config, &args.serve_log)
        .await
        .unwrap_or_else(|e| {
            error!("Failed to start server: {e}");
            std::process::exit(1);
        });
    let backend_port = serve.port();

    serve
        .wait_until_ready(backend_port, args.timeout)
        .await
        .unwrap_or_else(|e| {
            error!("{e}");
            error!(
                "Model loading hit timeout {}secs. Try increasing it.",
                args.timeout
            );
            std::process::exit(1);
        });

    // Construct KV manager once if enabled, restore if cache exists.
    let kv_mgr = if config.kv_cache {
        let mgr = Arc::new(kvcache::KvCacheManager::new(backend_port));
        match mgr.restore(&config).await {
            Ok(true) | Ok(false) => {} // already logged inside restore
            Err(e) => warn!("KV cache restore error (non-fatal): {e}"),
        }
        Some(mgr)
    } else {
        None
    };

    let summarizing = Arc::new(AtomicBool::new(false));

    // Populate runtime state from config
    let mut conversation = Conversation::new();
    for msg in &config.message {
        conversation.push(msg.clone());
    }
    let initial_summary = config.summary.clone();
    let initial_whispers = WhisperStack::from(config.whisper.clone());

    let config = Arc::new(config);
    let llm = Arc::new(client::LlmClient::new(backend_port, &config.model));
    let conversation = Arc::new(Mutex::new(conversation));
    let summary = Arc::new(Mutex::new(initial_summary));
    let whisper_stack = Arc::new(Mutex::new(initial_whispers));

    let mut retrieval_pools = Vec::new();
    for (name, pool_config) in &config.retrieval {
        if !pool_config.enabled {
            continue;
        }
        let pool_memories: Vec<_> = config
            .memory
            .iter()
            .filter(|m| m.enabled && m.pool == *name)
            .cloned()
            .collect();
        if pool_memories.is_empty() {
            continue;
        }
        info!(
            "[retrieval] pool '{}': {} memories",
            name,
            pool_memories.len()
        );
        retrieval_pools.push(NamedRetrieval {
            name: name.clone(),
            engine: RetrievalEngine::new(pool_config.clone(), &pool_memories),
        });
    }
    let retrieval_pools = Arc::new(retrieval_pools);

    let mut app = Router::new().route(
        "/ws",
        axum::routing::get({
            let conversation = conversation.clone();
            let llm = llm.clone();
            let config = config.clone();
            let summary = summary.clone();
            let whisper_stack = whisper_stack.clone();
            let retrieval_pools = retrieval_pools.clone();
            let kv_mgr = kv_mgr.clone();
            let summarizing = summarizing.clone();
            move |ws| {
                ws_handler(
                    ws,
                    conversation,
                    llm,
                    config,
                    summary,
                    whisper_stack,
                    retrieval_pools,
                    kv_mgr,
                    summarizing,
                )
            }
        }),
    );
    if args.dev {
        info!("[dev] Serving static/ from disk");
        app = app.fallback_service(tower_http::services::ServeDir::new("static"));
    } else {
        app = app
            .route("/", axum::routing::get(index))
            .route("/style.css", axum::routing::get(style))
            .route("/app.js", axum::routing::get(script));
    }

    let bind_addr = match config.frontend_port {
        Some(p) => format!("0.0.0.0:{p}"),
        None => "0.0.0.0:0".to_string(),
    };
    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            if let Some(p) = config.frontend_port {
                error!("Failed to bind frontend to port {p}: {e}");
                error!("Either change `frontend_port` in the TOML or free the port.");
            } else {
                error!("Failed to bind frontend: {e}");
            }
            std::process::exit(1);
        }
    };
    let frontend_port = listener.local_addr().unwrap().port();
    let local_url = format!("http://localhost:{frontend_port}");
    info!("Listening on {local_url}");

    println!("Local:     {local_url}");
    match tailscale::status().await {
        Ok(Some(info)) => {
            println!("Tailscale: http://{}:{}", info.ip, frontend_port);
            if let Some(dns) = &info.dns_name {
                println!("           http://{}:{}", dns, frontend_port);
            }
        }
        Ok(None) => {
            // binary missing — silent skip, already debug-logged
        }
        Err(e) => {
            eprintln!();
            eprintln!("⚠ {e}");
        }
    }

    if !args.no_open {
        open::that(&local_url).ok();
    }

    let shutdown_config = config.clone();
    let shutdown_conv = conversation.clone();
    let shutdown_summary = summary.clone();
    let shutdown_whispers = whisper_stack.clone();
    let shutdown_kv_mgr = kv_mgr.clone();
    let shutdown_summarizing = summarizing.clone();

    let shutdown = async {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutting down...");
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .unwrap();

    // Saving
    let snap = session::snapshot(
        &shutdown_config,
        &*shutdown_conv.lock().await,
        &*shutdown_summary.lock().await,
        &*shutdown_whispers.lock().await,
    );
    let save_path = session::save_path();
    match session::save(&snap, &save_path) {
        Ok(()) => info!("Session saved to {}", save_path.display()),
        Err(e) => error!("Failed to save session: {e}"),
    }

    // Save KV cache while server is still running.
    // Skip if a summary autogeneration was in flight — slot 0 holds mid-summary
    // garbage, but main.bin already holds the pre-summary good state.
    if shutdown_config.kv_cache && !shutdown_summarizing.load(Ordering::SeqCst) {
        if let Some(mgr) = &shutdown_kv_mgr {
            if let Err(e) = mgr.save(&shutdown_config).await {
                warn!("KV cache save failed (non-fatal): {e}");
            }
        }
    }
}

fn init_tracing(log_metrics: bool) {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{EnvFilter, filter, fmt};

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let console_layer = fmt::layer().with_filter(env_filter);

    let metrics_layer = if log_metrics {
        std::fs::create_dir_all("debug").ok();
        let ts = utils::file_timestamp();

        match std::fs::File::create(format!("debug/metrics_{ts}.log")) {
            Ok(file) => Some(
                fmt::layer()
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .with_target(false)
                    .with_filter(
                        filter::Targets::new().with_target("metrics", tracing::Level::TRACE),
                    ),
            ),
            Err(e) => {
                eprintln!("Failed to create metrics log: {e}");
                None
            }
        }
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(console_layer)
        .with(metrics_layer)
        .init();
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn style() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "text/css".parse().unwrap());
    (headers, include_str!("../static/style.css"))
}

async fn script() -> (axum::http::HeaderMap, &'static str) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", "application/javascript".parse().unwrap());
    (headers, include_str!("../static/app.js"))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    conversation: SharedConversation,
    llm: Arc<client::LlmClient>,
    config: Arc<ModelConfig>,
    summary: Arc<Mutex<SummaryState>>,
    whisper_stack: Arc<Mutex<WhisperStack>>,
    retrieval_pools: Arc<Vec<NamedRetrieval>>,
    kv_mgr: Option<Arc<kvcache::KvCacheManager>>,
    summarizing: Arc<AtomicBool>,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| {
        handle_socket(
            socket,
            conversation,
            llm,
            config,
            summary,
            whisper_stack,
            retrieval_pools,
            kv_mgr,
            summarizing,
        )
    })
}

async fn handle_socket(
    socket: WebSocket,
    conversation: chat::SharedConversation,
    llm: Arc<client::LlmClient>,
    config: Arc<ModelConfig>,
    summary: Arc<Mutex<SummaryState>>,
    whisper_stack: Arc<Mutex<WhisperStack>>,
    retrieval_pools: Arc<Vec<NamedRetrieval>>,
    kv_mgr: Option<Arc<kvcache::KvCacheManager>>,
    summarizing: Arc<AtomicBool>,
) {
    let mut conn = connection::Connection::new(
        socket,
        conversation,
        llm,
        config,
        summary,
        whisper_stack,
        retrieval_pools,
        kv_mgr,
        summarizing,
    );
    conn.run().await;
}
