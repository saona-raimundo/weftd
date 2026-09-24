// src/config.rs

use crate::chat::{Message, SummaryState};
use crate::whisper::Whisper;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Start,
    End,
}

/// Defines how Vec<Message> is rendered into a raw prompt string.
/// Each variant handles control tokens and structural constraints
/// for a model family's chat template.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptStructure {
    /// System messages anywhere. <|im_start|>/<|im_end|> tokens.
    /// Qwen, Mag-Mell, etc.
    #[serde(rename = "chatml", alias = "chat_m_l")]
    ChatML,
    /// Single [SYSTEM_PROMPT] block. Strict [INST]/[/INST] alternation.
    /// Magidonia, Cydonia, Magistral.
    MistralV7Tekken,
}

impl Default for PromptStructure {
    fn default() -> Self {
        Self::ChatML
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wrapper {
    pub position: Position,
    pub content: String,
    #[serde(default)]
    pub in_summarize: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub content: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: f64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_pool")]
    pub pool: String,
}

fn default_priority() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}

fn default_pool() -> String {
    "context".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryConfig {
    #[serde(default)]
    pub alert: Option<f64>,
    #[serde(default)]
    pub prompt: Option<String>,
}

impl SummaryConfig {
    /// Validate summary config at startup.
    pub fn validate(&self) {
        if let Some(alert) = self.alert {
            if !(0.0..=1.0).contains(&alert) {
                tracing::warn!(
                    "Summarize alert {alert} is outside [0.0, 1.0] — may never trigger or always trigger"
                );
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrievalConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_min_score")]
    pub min_score: f64,
    #[serde(default = "default_half")]
    pub keyword_weight: f64,
    #[serde(default = "default_half")]
    pub content_weight: f64,
    #[serde(default = "default_priority_weight")]
    pub priority_weight: f64,
}

fn default_max_results() -> usize {
    3
}

fn default_min_score() -> f64 {
    0.1
}

fn default_half() -> f64 {
    0.5
}

fn default_priority_weight() -> f64 {
    0.1
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_results: default_max_results(),
            min_score: default_min_score(),
            keyword_weight: default_half(),
            content_weight: default_half(),
            priority_weight: default_priority_weight(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SamplerConfig {
    // Standard
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    // Repetition control
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_last_n: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,

    // DRY sampler
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_multiplier: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_base: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_allowed_length: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_penalty_last_n: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_sequence_breakers: Option<Vec<String>>,

    // XTC sampler
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xtc_probability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xtc_threshold: Option<f64>,

    // Control
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samplers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_prompt: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cue {
    pub label: String,
    pub content: String,
    #[serde(default = "default_cue_turns")]
    pub turns: u32,
}

fn default_cue_turns() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub model: String,

    // Prompt format
    #[serde(default)]
    pub prompt_structure: PromptStructure,
    #[serde(default = "default_prompt_order")]
    pub prompt_order: Vec<String>,

    // Hardware
    #[serde(default = "default_threads")]
    pub threads: u32,
    #[serde(default)]
    pub ngl: u32,
    #[serde(default = "default_device")]
    pub device: String,
    #[serde(default = "default_no_mmap")]
    pub no_mmap: bool,

    // Runtime
    #[serde(default = "default_parallel")]
    pub parallel: u32,
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,
    #[serde(default)]
    pub cache_ram: u32,
    #[serde(default = "default_cache_reuse")]
    pub cache_reuse: u32,
    #[serde(default)]
    pub flash_attn: Option<bool>,

    // KV cache persistence
    #[serde(default)]
    pub kv_cache: bool,

    // Development
    #[serde(default)]
    pub log_prompt: bool,
    #[serde(default)]
    pub log_metrics: bool,

    // Server
    #[serde(default)]
    pub frontend_port: Option<u16>,

    // Context window
    #[serde(default = "default_recent_turns")]
    pub recent_turns: u32,

    // Generation
    #[serde(default = "default_ctx_size")]
    pub ctx_size: u32,
    #[serde(default)]
    pub thinking: bool,

    // Sampler
    #[serde(default)]
    pub sampler: SamplerConfig,

    // Summary
    #[serde(default)]
    pub summary_config: SummaryConfig,

    // Retrieval
    #[serde(default)]
    pub retrieval: HashMap<String, RetrievalConfig>,

    // Wrappers
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wrapper: Vec<Wrapper>,

    // Memory
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory: Vec<Memory>,

    // Conversation history
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message: Vec<Message>,

    // Runtime state
    #[serde(default)]
    pub summary: SummaryState,

    // Active whispers
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub whisper: Vec<Whisper>,

    // Cues
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cue: Vec<Cue>,
}

fn default_threads() -> u32 {
    6
}
fn default_device() -> String {
    "none".into()
}
fn default_no_mmap() -> bool {
    true
}
fn default_parallel() -> u32 {
    1
}
fn default_batch_size() -> u32 {
    512
}
fn default_ctx_size() -> u32 {
    8192
}

fn default_prompt_order() -> Vec<String> {
    vec![
        "start_wrappers".into(),
        "summary".into(),
        "recent_history".into(),
        "context".into(),
        "directive".into(),
        "end_wrappers".into(),
        "whispers".into(),
    ]
}

fn default_cache_reuse() -> u32 {
    256
}

fn default_recent_turns() -> u32 {
    5
}

impl Default for ModelConfig {
    fn default() -> Self {
        toml::from_str("").expect("ModelConfig defaults must be valid")
    }
}

impl ModelConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Role;

    #[test]
    fn parse_toml_with_memories_and_retrieval() {
        let toml_str = r#"
model = "hf://test"
threads = 4
ngl = 0
device = "none"
no_mmap = true
parallel = 1
batch_size = 512
cache_ram = 0
temperature = 1.0
min_p = 0.1
top_k = 40
repeat_pen = 1.1
ctx_size = 4096
max_tokens = 200
thinking = false

[retrieval.context]
enabled = true
max_results = 5
min_score = 0.2
keyword_weight = 0.6
content_weight = 0.3
priority_weight = 0.1

[[memory]]
content = "Elena swore to return the stolen map."
keywords = ["Elena", "map", "promise"]
priority = 5.0

[[memory]]
content = "The market in Mossford is run by Pip."
keywords = ["Mossford", "Pip"]

[[memory]]
enabled = false
content = "Elena visited the Sunken Temple."
keywords = ["Elena", "temple"]
priority = 2.0
"#;

        let config: ModelConfig = toml::from_str(toml_str).unwrap();

        assert!(config.retrieval["context"].enabled);
        assert_eq!(config.retrieval["context"].max_results, 5);
        assert!((config.retrieval["context"].min_score - 0.2).abs() < f64::EPSILON);
        assert!((config.retrieval["context"].keyword_weight - 0.6).abs() < f64::EPSILON);
        assert!((config.retrieval["context"].content_weight - 0.3).abs() < f64::EPSILON);
        assert!((config.retrieval["context"].priority_weight - 0.1).abs() < f64::EPSILON);

        assert_eq!(config.memory.len(), 3);
        assert_eq!(
            config.memory[0].content,
            "Elena swore to return the stolen map."
        );
        assert_eq!(config.memory[0].keywords, vec!["Elena", "map", "promise"]);
        assert!((config.memory[0].priority - 5.0).abs() < f64::EPSILON);
        assert!(config.memory[0].enabled);
        assert_eq!(
            config.memory[1].content,
            "The market in Mossford is run by Pip."
        );
        assert!((config.memory[1].priority - 1.0).abs() < f64::EPSILON);
        assert!(config.memory[1].enabled);
        assert!(!config.memory[2].enabled);
    }

    #[test]
    fn parse_toml_without_memories_or_retrieval() {
        let toml_str = r#"
model = "hf://test"
threads = 4
ngl = 0
device = "none"
no_mmap = true
parallel = 1
batch_size = 512
cache_ram = 0
temperature = 1.0
min_p = 0.1
top_k = 40
repeat_pen = 1.1
ctx_size = 4096
max_tokens = 200
thinking = false
"#;

        let config: ModelConfig = toml::from_str(toml_str).unwrap();
        assert!(config.memory.is_empty());
        assert!(config.retrieval.is_empty());
    }

    #[test]
    fn parse_toml_with_messages() {
        let toml_str = r#"
model = "hf://test"
threads = 4
ngl = 0
device = "none"
no_mmap = true
parallel = 1
batch_size = 512
cache_ram = 0
ctx_size = 4096
thinking = false

[[message]]
role = "system"
content = "Morning light through cheap curtains."

[[message]]
role = "assistant"
content = "She's been awake for eleven minutes."
"#;

        let config: ModelConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.message.len(), 2);
        assert_eq!(config.message[0].role, Role::System);
        assert_eq!(
            config.message[0].content,
            "Morning light through cheap curtains."
        );
        assert_eq!(config.message[1].role, Role::Assistant);
    }

    #[test]
    fn parse_toml_with_no_messages() {
        let toml_str = r#"
model = "hf://test"
threads = 4
ngl = 0
device = "none"
no_mmap = true
parallel = 1
batch_size = 512
cache_ram = 0
ctx_size = 4096
thinking = false
"#;

        let config: ModelConfig = toml::from_str(toml_str).unwrap();
        assert!(config.message.is_empty());
        assert!(config.summary.text.is_empty());
        assert_eq!(config.summary.up_to, 0);
        assert!(config.whisper.is_empty());
    }

    #[test]
    fn parse_toml_with_summary_and_whispers() {
        let toml_str = r#"
model = "hf://test"
threads = 4
ngl = 0
device = "none"
no_mmap = true
parallel = 1
batch_size = 512
cache_ram = 0
ctx_size = 4096
thinking = false

[summary]
text = "We argued about dinner."
up_to = 42

[[whisper]]
text = "keep the mood tense"
turns_remaining = 3
"#;

        let config: ModelConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.summary.text, "We argued about dinner.");
        assert_eq!(config.summary.up_to, 42);
        assert_eq!(config.whisper.len(), 1);
        assert_eq!(config.whisper[0].text, "keep the mood tense");
        assert_eq!(config.whisper[0].turns_remaining, 3);
    }

    #[test]
    fn round_trip_toml_serialization() {
        let toml_str = r#"
model = "hf://test"
threads = 4
ngl = 0
device = "none"
no_mmap = true
parallel = 1
batch_size = 512
cache_ram = 0
ctx_size = 4096
thinking = false

[[message]]
role = "system"
content = "Opening scene."

[[message]]
role = "assistant"
content = "I looked up."

[[message]]
role = "user"
content = "Hey."
"#;

        let config: ModelConfig = toml::from_str(toml_str).unwrap();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let reloaded: ModelConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(reloaded.message.len(), 3);
        assert_eq!(reloaded.message[0].role, Role::System);
        assert_eq!(reloaded.message[1].role, Role::Assistant);
        assert_eq!(reloaded.message[2].role, Role::User);
        assert_eq!(reloaded.message[2].content, "Hey.");
    }

    #[test]
    fn parse_toml_with_prompt_order() {
        let toml_str = r#"
    model = "hf://test"
    threads = 4
    ngl = 0
    device = "none"
    no_mmap = true
    parallel = 1
    batch_size = 512
    cache_ram = 0
    ctx_size = 4096
    thinking = false
    prompt_order = ["start_wrappers", "summary", "recent_history", "memories", "end_wrappers", "whispers"]
    "#;

        let config: ModelConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.prompt_order.len(), 6);
        assert_eq!(config.prompt_order[2], "recent_history");
    }

    #[test]
    fn parse_toml_without_prompt_order_gets_default() {
        let toml_str = r#"
    model = "hf://test"
    threads = 4
    ngl = 0
    device = "none"
    no_mmap = true
    parallel = 1
    batch_size = 512
    cache_ram = 0
    ctx_size = 4096
    thinking = false
    "#;

        let config: ModelConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.prompt_order,
            vec![
                "start_wrappers",
                "summary",
                "recent_history",
                "context",
                "directive",
                "end_wrappers",
                "whispers"
            ]
        );
    }

    #[test]
    fn parse_toml_with_dry_sequence_breakers() {
        let toml_str = r#"
    model = "hf://test"
    threads = 4
    ngl = 0
    device = "none"
    no_mmap = true
    parallel = 1
    batch_size = 512
    cache_ram = 0
    ctx_size = 4096
    thinking = false

    [sampler]
    dry_multiplier = 0.8
    dry_base = 1.75
    dry_allowed_length = 2
    dry_penalty_last_n = -1
    dry_sequence_breakers = ["\n", ":", "\"", "*"]
    "#;

        let config: ModelConfig = toml::from_str(toml_str).unwrap();
        let breakers = config.sampler.dry_sequence_breakers.unwrap();
        assert_eq!(breakers, vec!["\n", ":", "\"", "*"]);
    }
}
