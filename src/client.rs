// sre/client.rs

use crate::config::SamplerConfig;

use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, warn};

pub struct LlmClient {
    client: reqwest::Client,
    base_url: String,
}

impl LlmClient {
    pub fn new(port: u16, _model: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: format!("http://127.0.0.1:{port}"),
        }
    }
    pub async fn complete(
        &self,
        prompt: &str,
        sampler: &SamplerConfig,
    ) -> anyhow::Result<CompletionResult> {
        let request = CompletionRequest::new(prompt.to_string(), sampler, false);

        let response = self
            .client
            .post(format!("{}/completions", self.base_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM request failed ({status}): {body}");
        }

        let chunk: CompletionChunk = response.json().await?;
        let text = chunk.content;

        Ok(CompletionResult {
            text,
            timings: chunk.timings,
            time_to_first_token_ms: None,
            tokens_cached: chunk.tokens_cached,
            tokens_evaluated: chunk.tokens_evaluated,
        })
    }
    pub async fn complete_stream(
        &self,
        prompt: &str,
        sampler: &SamplerConfig,
        tx: mpsc::Sender<String>,
    ) -> anyhow::Result<CompletionResult> {
        let request = CompletionRequest::new(prompt.to_string(), sampler, true);

        let req = self
            .client
            .post(format!("{}/completions", self.base_url))
            .json(&request);

        let mut es = EventSource::new(req)?;
        let mut accumulated = String::new();
        let mut timings = None;
        let request_start = Instant::now();
        let mut first_token_at: Option<Instant> = None;
        let mut tokens_cached = None;
        let mut tokens_evaluated = None;

        while let Some(event) = es.next().await {
            match event {
                Ok(Event::Open) => {
                    debug!("[sse] connection opened");
                }
                Ok(Event::Message(msg)) => {
                    if msg.data == "[DONE]" {
                        break;
                    }
                    match serde_json::from_str::<CompletionChunk>(&msg.data) {
                        Ok(chunk) => {
                            if let Some(t) = chunk.timings {
                                timings = Some(t);
                            }
                            if let Some(tc) = chunk.tokens_cached {
                                tokens_cached = Some(tc);
                            }
                            if let Some(te) = chunk.tokens_evaluated {
                                tokens_evaluated = Some(te);
                            }
                            if !chunk.content.is_empty() {
                                if first_token_at.is_none() {
                                    first_token_at = Some(Instant::now());
                                }
                                accumulated.push_str(&chunk.content);
                                let _ = tx.send(accumulated.clone()).await;
                            }
                        }
                        Err(e) => {
                            warn!("[sse] failed to parse chunk: {e} — data: {}", msg.data);
                        }
                    }
                }
                Err(reqwest_eventsource::Error::StreamEnded) => {
                    break;
                }
                Err(e) => {
                    es.close();
                    anyhow::bail!("SSE stream error: {e}");
                }
            }
        }
        es.close();

        let time_to_first_token_ms =
            first_token_at.map(|t| (t - request_start).as_secs_f64() * 1000.0);

        Ok(CompletionResult {
            text: accumulated,
            timings,
            time_to_first_token_ms,
            tokens_cached,
            tokens_evaluated,
        })
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Timings {
    pub prompt_n: Option<u32>,
    pub prompt_ms: Option<f64>,
    pub prompt_per_second: Option<f64>,
    pub predicted_n: Option<u32>,
    pub predicted_ms: Option<f64>,
    pub predicted_per_second: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct CompletionResult {
    pub text: String,
    pub timings: Option<Timings>,
    pub time_to_first_token_ms: Option<f64>,
    pub tokens_cached: Option<u32>,
    pub tokens_evaluated: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct CompletionRequest {
    pub prompt: String,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n_predict: Option<i32>,
    #[serde(flatten)]
    pub sampler: SamplerConfig,
}

impl CompletionRequest {
    pub fn new(prompt: String, sampler: &SamplerConfig, stream: bool) -> Self {
        let mut sampler = sampler.clone();
        let n_predict = sampler.max_tokens.take(); // moves value out, max_tokens becomes None (skipped)
        Self {
            prompt,
            stream,
            n_predict,
            sampler,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CompletionChunk {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timings: Option<Timings>,
    #[serde(default)]
    pub tokens_cached: Option<u32>,
    #[serde(default)]
    pub tokens_evaluated: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_complete_stream_accumulates_and_sends() {
        // Test the channel/accumulation pattern in isolation
        // by simulating what complete_stream does internally
        let (tx, mut rx) = mpsc::channel::<String>(16);

        let producer = tokio::spawn(async move {
            let words = ["The ", "dragon ", "circled."];
            let mut accumulated = String::new();
            for word in words {
                accumulated.push_str(word);
                let _ = tx.send(accumulated.clone()).await;
            }
            accumulated
        });

        let mut last = String::new();
        while let Some(text) = rx.recv().await {
            // Each message should be longer than the last
            assert!(text.len() >= last.len());
            last = text;
        }

        let final_text = producer.await.unwrap();
        assert_eq!(final_text, "The dragon circled.");
        assert_eq!(last, "The dragon circled.");
    }

    #[test]
    fn request_omits_none_fields() {
        let sampler = SamplerConfig::default();
        let req = CompletionRequest::new("test".into(), &sampler, true);
        let json = serde_json::to_value(&req).unwrap();

        // Always present
        assert!(json.get("prompt").is_some());
        assert!(json.get("stream").is_some());

        // All None — should be absent
        assert!(json.get("temperature").is_none());
        assert!(json.get("dry_multiplier").is_none());
        assert!(json.get("samplers").is_none());
    }

    #[test]
    fn request_includes_set_fields() {
        let sampler = SamplerConfig {
            temperature: Some(1.05),
            dry_multiplier: Some(0.8),
            max_tokens: Some(200),
            stop: Some(vec!["</s>".into()]),
            ..Default::default()
        };
        let req = CompletionRequest::new("test".into(), &sampler, true);
        let json = serde_json::to_value(&req).unwrap();

        assert_eq!(json["temperature"], 1.05);
        assert_eq!(json["dry_multiplier"], 0.8);
        assert_eq!(json["n_predict"], 200);
        assert_eq!(json["stop"][0], "</s>");
        // max_tokens should NOT appear (taken by n_predict)
        assert!(json.get("max_tokens").is_none());
        assert!(json.get("seed").is_none());
    }

    #[test]
    fn parse_chunk() {
        let data = r#"{"content":" hello"}"#;
        let chunk: CompletionChunk = serde_json::from_str(data).unwrap();
        assert_eq!(chunk.content, " hello");
    }

    #[test]
    fn parse_chunk_ignores_extra_fields() {
        let data = r#"{"content":" world","id_slot":0,"stop":true,"model":"test","tokens_cached":500,"tokens_evaluated":42}"#;
        let chunk: CompletionChunk = serde_json::from_str(data).unwrap();
        assert_eq!(chunk.content, " world");
        assert_eq!(chunk.tokens_cached, Some(500));
        assert_eq!(chunk.tokens_evaluated, Some(42));
    }
    #[test]
    fn parse_chunk_with_timings() {
        let data = r#"{"content":"","stop":true,"timings":{"prompt_n":100,"prompt_ms":500.0,"prompt_per_second":200.0,"predicted_n":50,"predicted_ms":1000.0,"predicted_per_second":50.0},"tokens_cached":80,"tokens_evaluated":20}"#;
        let chunk: CompletionChunk = serde_json::from_str(data).unwrap();
        assert!(chunk.timings.is_some());
        assert_eq!(chunk.tokens_cached, Some(80));
        assert_eq!(chunk.tokens_evaluated, Some(20));
    }
}
