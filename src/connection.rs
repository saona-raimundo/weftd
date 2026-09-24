// src/connection.rs

use crate::chat::{self, Message, Role, SharedConversation, SummaryState};
use crate::client::{self, CompletionResult};
use crate::command;
use crate::config::ModelConfig;
use crate::execute;
use crate::kvcache::KvCacheManager;
use crate::prompt;
use crate::render;
use crate::retrieval::NamedRetrieval;
use crate::summary;
use crate::utils;
use crate::whisper::WhisperStack;

use axum::extract::ws::{Message as WsMessage, WebSocket};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

#[derive(Debug)]
struct TurnMetrics {
    retrieval_ms: f64,
    llm_wall_ms: f64,
    post_generation_ms: f64,
    completion: CompletionResult,
}

impl TurnMetrics {
    fn log(&self) {
        let t = &self.completion.timings;
        let overhead = self.retrieval_ms + self.post_generation_ms;
        let prompt_n = t.as_ref().and_then(|t| t.prompt_n).unwrap_or(0);
        let prompt_ms = t.as_ref().and_then(|t| t.prompt_ms).unwrap_or(0.0);
        let prompt_tps = t.as_ref().and_then(|t| t.prompt_per_second).unwrap_or(0.0);
        let predicted_n = t.as_ref().and_then(|t| t.predicted_n).unwrap_or(0);
        let predicted_ms = t.as_ref().and_then(|t| t.predicted_ms).unwrap_or(0.0);
        let predicted_tps = t
            .as_ref()
            .and_then(|t| t.predicted_per_second)
            .unwrap_or(0.0);
        let ttft = self.completion.time_to_first_token_ms.unwrap_or(0.0);

        tracing::trace!(
            target: "metrics",
            retrieval_ms = self.retrieval_ms,
            llm_wall_ms = self.llm_wall_ms,
            overhead_ms = overhead,
            cache_miss = prompt_n, // tokens that were actually evaluated (not cached)
            prompt_ms,
            prompt_tps,
            predicted_n,
            predicted_ms,
            predicted_tps,
            ttft_ms = ttft,
            tokens_cached = self.completion.tokens_cached.unwrap_or(0),
            tokens_evaluated = self.completion.tokens_evaluated.unwrap_or(0),
        );
    }
}

pub struct Connection {
    socket: WebSocket,
    conversation: SharedConversation,
    llm: Arc<client::LlmClient>,
    config: Arc<ModelConfig>,
    summary: Arc<Mutex<SummaryState>>,
    whisper_stack: Arc<Mutex<WhisperStack>>,
    retrievals: Arc<Vec<NamedRetrieval>>,
    kv_cache: Option<Arc<KvCacheManager>>,
    summarizing: Arc<AtomicBool>,
    last_submit_id: Option<String>,
}

impl Connection {
    pub fn new(
        socket: WebSocket,
        conversation: SharedConversation,
        llm: Arc<client::LlmClient>,
        config: Arc<ModelConfig>,
        summary: Arc<Mutex<SummaryState>>,
        whisper_stack: Arc<Mutex<WhisperStack>>,
        retrievals: Arc<Vec<NamedRetrieval>>,
        kv_cache: Option<Arc<KvCacheManager>>,
        summarizing: Arc<AtomicBool>,
    ) -> Self {
        Self {
            socket,
            conversation,
            llm,
            config,
            summary,
            whisper_stack,
            retrievals,
            kv_cache,
            summarizing,
            last_submit_id: None,
        }
    }
    pub async fn run(&mut self) {
        // Send initial state so the frontend renders any pre-populated conversation
        self.send_sync().await;
        self.send_whispers().await;
        self.send_cues().await;

        while let Some(Ok(msg)) = self.socket.recv().await {
            if let WsMessage::Text(text) = msg {
                match serde_json::from_str::<chat::Action>(&text) {
                    Ok(chat::Action::Submit {
                        role,
                        content,
                        submit_id,
                    }) => {
                        if submit_id.is_some() {
                            self.last_submit_id = submit_id;
                        }
                        match command::parse(&content) {
                            command::ParseResult::Fiction => {
                                debug!("[{role}] {content}");
                                let msg = Message { role, content };
                                self.conversation.lock().await.push(msg.clone());
                                self.send_sync().await;
                                self.generate_turn().await;
                            }
                            command::ParseResult::Command(cmd) => {
                                let effect = {
                                    let mut conv = self.conversation.lock().await;
                                    let mut whispers = self.whisper_stack.lock().await;
                                    execute::dispatch(cmd, &mut conv, &mut whispers)
                                };
                                self.apply(effect).await;
                            }
                            command::ParseResult::Unknown(raw) => {
                                warn!("[unknown command] {raw}");
                                self.send_error(format!("Unknown command: {raw}")).await;
                            }
                        }
                        if let Some(id) = self.last_submit_id.take() {
                            self.send_ack(&id).await;
                        }
                    }
                    Ok(chat::Action::Edit { index, content }) => {
                        debug!("[edit {index}] {content}");
                        self.conversation.lock().await.edit(index, content);
                        self.send_sync().await;
                    }
                    Ok(chat::Action::SummaryEdit { text, up_to }) => {
                        let mut s = self.summary.lock().await;
                        s.text = text;
                        s.up_to = up_to;
                        drop(s);
                        self.send_summary().await;
                    }
                    Ok(chat::Action::SummaryTrigger) => {
                        self.run_summary().await;
                    }
                    Err(e) => {
                        warn!("Invalid message: {e}");
                    }
                }
            }
        }
    }

    async fn generate(
        &mut self,
        history: &[Message],
        task_outputs: &HashMap<String, Vec<Message>>,
    ) -> Option<CompletionResult> {
        self.send(json!({"event": "thinking"})).await;

        let current_summary = self.summary.lock().await.clone();
        let whisper_messages = self.whisper_stack.lock().await.inject();

        let full_prompt = prompt::build(
            &self.config,
            history,
            &current_summary,
            task_outputs,
            &whisper_messages,
        );

        let config = self.config.clone();
        let rendered = render::render(&config.prompt_structure, &full_prompt);

        if self.config.log_prompt {
            Self::dump_prompt(&rendered);
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(16);
        let llm = self.llm.clone();

        let gen_task =
            tokio::spawn(async move { llm.complete_stream(&rendered, &config.sampler, tx).await });

        while let Some(text_so_far) = rx.recv().await {
            self.send(json!({
                "event": "token",
                "content": text_so_far
            }))
            .await;
        }

        match gen_task.await {
            Ok(Ok(result)) => {
                debug!("[assistant] {}", result.text);

                self.conversation.lock().await.push(Message {
                    role: Role::Assistant,
                    content: result.text.clone(),
                });

                let had_whispers = {
                    let mut stack = self.whisper_stack.lock().await;
                    if !stack.is_empty() {
                        stack.tick();
                        true
                    } else {
                        false
                    }
                };
                if had_whispers {
                    self.send_whispers().await;
                }

                Some(result)
            }
            Ok(Err(e)) => {
                error!("LLM error: {e}");
                self.send_error(format!("{e}")).await;
                None
            }
            Err(e) => {
                error!("LLM task panicked: {e}");
                self.send_error("Generation task failed".into()).await;
                None
            }
        }
    }

    // async fn maybe_summarize(&mut self) {
    //     let conv = self.conversation.lock().await;
    //     let turn_count = conv.user_turn_count();

    //     if !summary::should_run(turn_count, self.config.summarize_every) {
    //         return;
    //     }

    //     let history = conv.messages().to_vec();
    //     drop(conv);

    //     self.send(json!({"event": "summarizing"})).await;

    //     let current_summary = self.summary.lock().await.clone();
    //     match summary::run(&self.config, &current_summary, &history, &self.llm).await {
    //         Ok(output) => {
    //             let mut state = self.summary.lock().await;
    //             state.text = output.text;
    //             state.up_to = output.up_to;
    //         }
    //         Err(e) => {
    //             error!("Summarize error: {e}");
    //         }
    //     }

    //     self.send(json!({"event": "summarized"})).await;
    // }

    /// Check prompt-to-context ratio and fire alert and/or auto-summarization.
    async fn check_summary_thresholds(&mut self, result: &CompletionResult) {
        let ratio = match summary::prompt_ratio(
            result.tokens_cached,
            result.tokens_evaluated,
            self.config.ctx_size,
        ) {
            Some(r) => r,
            None => return, // no token data available
        };

        // Alert fires at lower threshold
        if summary::should_alert(&self.config, ratio) {
            self.send(json!({
                "event": "summary_needed",
                "ratio": ratio
            }))
            .await;
        }
    }

    async fn send(&mut self, event: Value) {
        let _ = self
            .socket
            .send(WsMessage::Text(event.to_string().into()))
            .await;
    }

    async fn send_whispers(&mut self) {
        let stack = self.whisper_stack.lock().await;
        let active: Vec<_> = stack
            .active()
            .iter()
            .map(|w| {
                json!({
                    "text": w.text,
                    "turns_remaining": w.turns_remaining
                })
            })
            .collect();
        drop(stack);
        self.send(json!({
            "event": "whispers",
            "active": active
        }))
        .await;
    }

    async fn send_cues(&mut self) {
        let entries: Vec<_> = self
            .config
            .cue
            .iter()
            .map(|c| {
                json!({
                    "label": c.label,
                    "content": c.content,
                    "turns": c.turns
                })
            })
            .collect();
        self.send(json!({
            "event": "cues",
            "entries": entries
        }))
        .await;
    }

    async fn send_sync(&mut self) {
        let conv = self.conversation.lock().await;
        let messages: Vec<_> = conv
            .messages()
            .iter()
            .map(|m| {
                json!({
                    "role": m.role.to_string(),
                    "content": m.content
                })
            })
            .collect();
        drop(conv);
        let s = self.summary.lock().await;
        let summary_text = s.text.clone();
        let summary_up_to = s.up_to;
        drop(s);
        self.send(json!({
            "event": "sync",
            "messages": messages,
            "summary": {
                "text": summary_text,
                "up_to": summary_up_to
            },
            "submit_id": self.last_submit_id
        }))
        .await;
    }

    async fn send_ack(&mut self, submit_id: &str) {
        self.send(json!({
            "event": "ack",
            "submit_id": submit_id
        }))
        .await;
    }

    async fn send_error(&mut self, content: String) {
        self.send(json!({
            "event": "error",
            "content": content,
            "submit_id": self.last_submit_id
        }))
        .await;
    }

    async fn apply(&mut self, effect: execute::Effect) {
        let effects = match effect {
            execute::Effect::Multi(effects) => effects,
            single => vec![single],
        };

        for effect in effects {
            match effect {
                execute::Effect::Sync => {
                    self.send_sync().await;
                }
                execute::Effect::Generate => {
                    self.generate_turn().await;
                }
                execute::Effect::WhispersChanged => {
                    self.send_whispers().await;
                }
                execute::Effect::Error(msg) => {
                    self.send_error(msg).await;
                }
                execute::Effect::Multi(_) => {
                    unreachable!("nested Multi effects are not supported");
                }
            }
        }
    }

    fn dump_prompt(rendered: &str) {
        let ts = utils::file_timestamp();

        let filename = format!("debug/prompt_{ts}.txt");
        if let Err(e) = std::fs::create_dir_all("debug") {
            warn!("failed to create debug/ directory: {e}");
            return;
        }
        match std::fs::write(&filename, rendered) {
            Ok(()) => debug!("wrote prompt dump to {filename}"),
            Err(e) => warn!("failed to write prompt dump: {e}"),
        }
    }

    /// Full turn pipeline: pre-generation tasks → generate → post-generation tasks.
    async fn generate_turn(&mut self) {
        let history = self.conversation.lock().await.messages().to_vec();
        let log_metrics = self.config.log_metrics;

        // Pre-generation tasks
        let t0 = if log_metrics {
            Some(Instant::now())
        } else {
            None
        };
        let mut task_outputs: HashMap<String, Vec<Message>> = HashMap::new();
        for pool in self.retrievals.iter() {
            let hits = pool.engine.run(&history, self.config.recent_turns);
            if hits.is_empty() {
                continue;
            }
            let content = format!("[{}]\n{}\n[/{}]", pool.name, hits.join("\n"), pool.name);
            task_outputs.insert(
                pool.name.clone(),
                vec![Message {
                    role: Role::System,
                    content,
                }],
            );
        }
        let t1 = if log_metrics {
            Some(Instant::now())
        } else {
            None
        };

        // Generation
        let result = self.generate(&history, &task_outputs).await;
        let t2 = if log_metrics {
            Some(Instant::now())
        } else {
            None
        };
        self.send_sync().await;

        // Post-generation tasks
        if let Some(ref result) = result {
            self.check_summary_thresholds(result).await;
        }
        let t3 = if log_metrics {
            Some(Instant::now())
        } else {
            None
        };

        if log_metrics {
            if let (Some(result), Some(t0), Some(t1), Some(t2), Some(t3)) = (result, t0, t1, t2, t3)
            {
                let metrics = TurnMetrics {
                    retrieval_ms: (t1 - t0).as_secs_f64() * 1000.0,
                    llm_wall_ms: (t2 - t1).as_secs_f64() * 1000.0,
                    post_generation_ms: (t3 - t2).as_secs_f64() * 1000.0,
                    completion: result,
                };
                metrics.log();
            }
        }
    }

    async fn send_summary(&mut self) {
        let s = self.summary.lock().await;
        let text = s.text.clone();
        let up_to = s.up_to;
        drop(s);
        self.send(json!({
            "event": "summary",
            "text": text,
            "up_to": up_to
        }))
        .await;
    }

    async fn run_summary(&mut self) {
        if self.config.summary_config.prompt.is_none() {
            self.send_error("summary_config.prompt must be set for autogeneration".into())
                .await;
            return;
        }

        let history = self.conversation.lock().await.messages().to_vec();
        let current = self.summary.lock().await.clone();

        self.send(json!({ "event": "summarizing" })).await;
        self.summarizing.store(true, Ordering::SeqCst);

        // Save slot 0 before summary corrupts it. Save failure aborts —
        // we'd have nothing to restore from.
        let saved = if let Some(mgr) = &self.kv_cache {
            match mgr.save(&self.config).await {
                Ok(()) => true,
                Err(e) => {
                    warn!("KV cache save failed, aborting summary: {e}");
                    self.send_error(format!("Could not save KV cache, summary aborted: {e}"))
                        .await;
                    self.summarizing.store(false, Ordering::SeqCst);
                    self.send(json!({ "event": "summarized" })).await;
                    return;
                }
            }
        } else {
            false
        };

        let result = summary::run(&self.config, &current, &history, &self.llm).await;

        // Restore unconditionally — slot 0 is corrupted whether summary succeeded or not.
        if saved {
            if let Some(mgr) = &self.kv_cache {
                match mgr.restore(&self.config).await {
                    Ok(true) => {}
                    Ok(false) => warn!("KV cache restore returned false after summary"),
                    Err(e) => warn!("KV cache restore failed after summary: {e}"),
                }
            }
        }

        self.summarizing.store(false, Ordering::SeqCst);

        match result {
            Ok(new_state) => {
                *self.summary.lock().await = new_state;
                self.send_summary().await;
            }
            Err(e) => {
                self.send_error(format!("Summarization failed: {e}")).await;
            }
        }

        self.send(json!({ "event": "summarized" })).await;
    }
}
