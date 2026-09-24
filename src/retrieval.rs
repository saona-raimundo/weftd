// src/retrieval.rs

use crate::chat::Message;
use crate::config::{Memory, RetrievalConfig};

use bm25::{Document, Language, SearchEngine, SearchEngineBuilder};
use std::collections::HashMap;
use tracing::debug;
use yake_rust::{Config as YakeConfig, StopWords};

/// Internal representation of an enabled memory, ready for indexing.
#[derive(Debug)]
struct IndexedMemory {
    index: usize, // original position in the config's memory vec (used as BM25 doc ID)
    content: String,
    keywords: String, // pre-joined keywords, empty string if none provided
    priority: f64,
}

/// Immutable after construction. Owns two BM25 indices and the filtered memory list.
/// Shared via Arc — no mutex needed.
pub struct RetrievalEngine {
    config: RetrievalConfig,
    memories: Vec<IndexedMemory>,
    content_engine: SearchEngine<usize>,
    keyword_engine: SearchEngine<usize>,
    max_priority: f64,
    stop_words: StopWords,
    yake_config: YakeConfig,
}

impl RetrievalEngine {
    /// Build from config and raw memory list. Filters disabled entries, builds both BM25 indices.
    pub fn new(config: RetrievalConfig, raw_memories: &[Memory]) -> Self {
        // Filter disabled memories before building indices — keeps IDF stats clean
        let memories: Vec<IndexedMemory> = raw_memories
            .iter()
            .enumerate()
            .filter(|(_, m)| m.enabled)
            .map(|(i, m)| IndexedMemory {
                index: i,
                content: m.content.clone(),
                keywords: m.keywords.join(" "),
                priority: m.priority,
            })
            .collect();

        // Floor at 1.0 to avoid division by zero when all priorities are equal
        let max_priority = memories
            .iter()
            .map(|m| m.priority)
            .fold(0.0_f64, f64::max)
            .max(1.0);

        let (content_engine, keyword_engine) = if memories.is_empty() {
            // Empty engines that return no results — avoids special-casing everywhere
            (
                SearchEngineBuilder::<usize>::with_avgdl(20.0).build(),
                SearchEngineBuilder::<usize>::with_avgdl(20.0).build(),
            )
        } else {
            let content_docs: Vec<Document<usize>> = memories
                .iter()
                .map(|m| Document {
                    id: m.index,
                    contents: m.content.clone(),
                })
                .collect();

            let keyword_docs: Vec<Document<usize>> = memories
                .iter()
                .map(|m| Document {
                    id: m.index,
                    // Fall back to content when no keywords — prevents this memory
                    // from being invisible to keyword-weighted scoring
                    contents: if m.keywords.is_empty() {
                        m.content.clone()
                    } else {
                        m.keywords.clone()
                    },
                })
                .collect();

            let ce = SearchEngineBuilder::with_documents(Language::English, content_docs).build();
            let ke = SearchEngineBuilder::with_documents(Language::English, keyword_docs).build();
            (ce, ke)
        };

        // YAKE: unigrams only — fiction keywords are predominantly proper nouns
        let stop_words = StopWords::predefined("en")
            .unwrap_or_else(|| StopWords::custom(std::collections::HashSet::new()));
        let yake_config = YakeConfig {
            ngrams: 1,
            ..YakeConfig::default()
        };

        Self {
            config,
            memories,
            content_engine,
            keyword_engine,
            max_priority,
            stop_words,
            yake_config,
        }
    }

    /// True if retrieval is enabled and there are memories to search.
    pub fn is_active(&self) -> bool {
        self.config.enabled && !self.memories.is_empty()
    }

    /// Run the retrieval task for this turn. Extracts keywords from recent
    /// history, scores all memories, returns the top results.
    pub fn run(&self, history: &[Message], recent_turns: u32) -> Vec<String> {
        if !self.is_active() {
            return Vec::new();
        }

        let start = history.len().saturating_sub(recent_turns as usize);
        let recent_slice = &history[start..];
        let memories = self.retrieve(recent_slice);

        if !memories.is_empty() {
            debug!("[retrieval] injecting {} memories", memories.len());
        }

        memories
    }

    /// Extract keywords from recent conversation messages using YAKE.
    /// Concatenates all messages into a single text block for extraction.
    fn extract_keywords(&self, recent_messages: &[Message]) -> Vec<String> {
        let text: String = recent_messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        if text.trim().is_empty() {
            return Vec::new();
        }

        let results = yake_rust::get_n_best(10, &text, &self.stop_words, &self.yake_config);
        results.into_iter().map(|r| r.keyword).collect()
    }

    /// Retrieve the most relevant memories for the current conversation context.
    ///
    /// `recent_messages` should be the same recent-turns slice used by prompt building.
    pub fn retrieve(&self, recent_messages: &[Message]) -> Vec<String> {
        if !self.is_active() {
            return Vec::new();
        }

        let keywords = self.extract_keywords(recent_messages);
        if keywords.is_empty() {
            return Vec::new();
        }

        let query = keywords.join(" ");
        let limit = self.memories.len(); // score all candidates, filter later

        // Query both indices
        let content_results = self.content_engine.search(&query, limit);
        let keyword_results = self.keyword_engine.search(&query, limit);

        // Build score maps: memory index → BM25 score
        let content_scores: HashMap<usize, f64> = content_results
            .iter()
            .map(|r| (r.document.id, r.score as f64))
            .collect();
        let keyword_scores: HashMap<usize, f64> = keyword_results
            .iter()
            .map(|r| (r.document.id, r.score as f64))
            .collect();

        // Composite scoring
        let mut scored: Vec<(usize, f64)> = self
            .memories
            .iter()
            .map(|m| {
                let cs = content_scores.get(&m.index).copied().unwrap_or(0.0);
                let ks = keyword_scores.get(&m.index).copied().unwrap_or(0.0);
                let ps = m.priority / self.max_priority;

                let score = self.config.content_weight * cs
                    + self.config.keyword_weight * ks
                    + self.config.priority_weight * ps;

                (m.index, score)
            })
            .filter(|(_, score)| *score >= self.config.min_score)
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Take top-K
        scored
            .iter()
            .take(self.config.max_results)
            .map(|(idx, _score)| {
                let mem = self.memories.iter().find(|m| m.index == *idx).unwrap();
                mem.content.clone()
            })
            .collect()
    }
}

/// A named retrieval pool, built at startup and iterated each turn.
/// Immutable after construction.
pub struct NamedRetrieval {
    pub name: String,
    pub engine: RetrievalEngine,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Role;
    use crate::config::{Memory, RetrievalConfig};

    fn mem(content: &str, keywords: &[&str], priority: f64) -> Memory {
        Memory {
            content: content.to_string(),
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            priority,
            enabled: true,
            pool: "test".into(),
        }
    }

    fn msg(content: &str) -> Message {
        Message {
            role: Role::User,
            content: content.to_string(),
        }
    }

    #[test]
    fn empty_memories_returns_nothing() {
        let engine = RetrievalEngine::new(RetrievalConfig::default(), &[]);
        assert!(!engine.is_active());
        assert!(engine.retrieve(&[msg("hello")]).is_empty());
    }

    #[test]
    fn disabled_memories_filtered_out() {
        let memories = vec![Memory {
            content: "Elena has a map".to_string(),
            keywords: vec!["Elena".to_string()],
            priority: 1.0,
            enabled: false,
            pool: String::new(),
        }];
        let engine = RetrievalEngine::new(RetrievalConfig::default(), &memories);
        assert!(!engine.is_active());
    }

    #[test]
    fn content_only_retrieval() {
        let memories = vec![
            mem("Elena swore to return the stolen map to Aldric", &[], 1.0),
            mem(
                "The market in Mossford is run by Pip the halfling",
                &[],
                1.0,
            ),
        ];
        let config = RetrievalConfig {
            content_weight: 1.0,
            keyword_weight: 0.0,
            priority_weight: 0.0,
            min_score: 0.0,
            ..RetrievalConfig::default()
        };

        let engine = RetrievalEngine::new(config, &memories);
        let results = engine.retrieve(&[msg("Elena promised to return the map")]);

        assert!(!results.is_empty());
        assert!(results[0].contains("Elena"));
    }

    #[test]
    fn keyword_only_retrieval() {
        let memories = vec![
            mem(
                "She fears deep water since childhood",
                &["Elena", "water", "fear"],
                1.0,
            ),
            mem(
                "The market has exotic spices",
                &["Mossford", "market", "spices"],
                1.0,
            ),
        ];
        let config = RetrievalConfig {
            content_weight: 0.0,
            keyword_weight: 1.0,
            priority_weight: 0.0,
            min_score: 0.0,
            ..RetrievalConfig::default()
        };

        let engine = RetrievalEngine::new(config, &memories);
        let results = engine.retrieve(&[msg("Elena approached the river with dread")]);

        assert!(!results.is_empty());
        assert!(results[0].contains("water"));
    }

    #[test]
    fn priority_boosting() {
        let memories = vec![
            mem("A minor detail about the town", &["town"], 1.0),
            mem("A critical plot point about the town", &["town"], 10.0),
        ];
        let config = RetrievalConfig {
            content_weight: 0.0,
            keyword_weight: 0.5,
            priority_weight: 0.5,
            min_score: 0.0,
            ..RetrievalConfig::default()
        };

        let engine = RetrievalEngine::new(config, &memories);
        let results = engine.retrieve(&[msg("What about the town?")]);

        assert_eq!(results.len(), 2);
        assert!(results[0].contains("critical"));
    }

    #[test]
    fn max_results_caps_output() {
        let memories = vec![
            mem("Memory one about alpha", &["alpha"], 1.0),
            mem("Memory two about alpha", &["alpha"], 1.0),
            mem("Memory three about alpha", &["alpha"], 1.0),
        ];
        let config = RetrievalConfig {
            max_results: 2,
            min_score: 0.0,
            ..RetrievalConfig::default()
        };

        let engine = RetrievalEngine::new(config, &memories);
        let results = engine.retrieve(&[msg("Tell me about alpha")]);

        assert!(results.len() <= 2);
    }

    #[test]
    fn min_score_filtering() {
        let memories = vec![mem("Elena has a sword", &["Elena", "sword"], 1.0)];
        let config = RetrievalConfig {
            min_score: 9999.0,
            ..RetrievalConfig::default()
        };

        let engine = RetrievalEngine::new(config, &memories);
        let results = engine.retrieve(&[msg("Elena drew her sword")]);

        assert!(results.is_empty());
    }

    /// End-to-end test: config → engine → retrieve → inject.
    /// Mirrors how connection.rs uses the engine.
    #[test]
    fn end_to_end_config_to_injection() {
        // Simulate a realistic TOML config with mixed memories
        let memories = vec![
            mem(
                "Elena swore to return the stolen map to Aldric, no matter the cost.",
                &["Elena", "map", "Aldric", "promise", "honor"],
                5.0,
            ),
            mem(
                "Elena is terrified of deep water since nearly drowning as a child.",
                &["Elena", "water", "fear", "drowning"],
                3.0,
            ),
            mem(
                "The market in Mossford is run by Pip who knows everyone's secrets.",
                &["Mossford", "market", "Pip", "secrets"],
                1.0,
            ),
            // Disabled — should never appear
            Memory {
                content: "Elena visited the Sunken Temple and found a golden compass.".to_string(),
                keywords: vec![
                    "Elena".to_string(),
                    "temple".to_string(),
                    "compass".to_string(),
                ],
                priority: 10.0, // high priority, but disabled
                enabled: false,
                pool: String::new(),
            },
        ];

        let config = RetrievalConfig {
            max_results: 2,
            min_score: 0.0,
            keyword_weight: 0.5,
            content_weight: 0.5,
            priority_weight: 0.1,
            ..RetrievalConfig::default()
        };

        let engine = RetrievalEngine::new(config, &memories);
        assert!(engine.is_active());

        // Simulate a recent conversation about Elena and the map
        let recent = vec![
            msg("Elena unfolded the map on the tavern table."),
            Message {
                role: Role::Assistant,
                content: "The barkeep glanced at the markings. \"That's Aldric's hand,\" he said quietly.".to_string(),
            },
            msg("\"I made him a promise,\" Elena replied."),
        ];

        let retrieved = engine.retrieve(&recent);
        // Verify we got the right memories back
        assert!(!retrieved.is_empty());
        assert!(retrieved.iter().any(|r| r.contains("stolen map")));
    }
}
