use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

#[cfg(feature = "fastembed-backend")]
use anyhow::Context;
use anyhow::{Result, bail};
#[cfg(feature = "fastembed-backend")]
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::config::AppConfig;

#[derive(Clone)]
pub struct EmbeddingEngine {
    backend: Arc<BackendImpl>,
}

enum BackendImpl {
    Hashing(HashingBackend),
    #[cfg(feature = "fastembed-backend")]
    Fastembed(FastembedBackend),
}

impl EmbeddingEngine {
    /// 根据配置初始化向量后端；支持 hashing 与 fastembed 两种实现。
    pub fn new(config: &AppConfig) -> Result<Self> {
        let backend = match config.embedding_backend.as_str() {
            "hashing" => {
                if config.hashing_dimensions == 0 {
                    bail!("hashing_dimensions must be greater than zero");
                }
                BackendImpl::Hashing(HashingBackend::new(config.hashing_dimensions))
            }
            "fastembed" => {
                #[cfg(feature = "fastembed-backend")]
                {
                    BackendImpl::Fastembed(FastembedBackend::new(config)?)
                }
                #[cfg(not(feature = "fastembed-backend"))]
                {
                    bail!("embedding_backend=fastembed requires Cargo feature `fastembed-backend`");
                }
            }
            other => bail!("unsupported embedding_backend: {other}"),
        };

        Ok(Self {
            backend: Arc::new(backend),
        })
    }

    pub fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        match self.backend.as_ref() {
            BackendImpl::Hashing(backend) => Ok(backend.embed_texts(texts)),
            #[cfg(feature = "fastembed-backend")]
            BackendImpl::Fastembed(backend) => backend.embed_passages(texts),
        }
    }

    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        match self.backend.as_ref() {
            BackendImpl::Hashing(backend) => Ok(backend.embed_text(query)),
            #[cfg(feature = "fastembed-backend")]
            BackendImpl::Fastembed(backend) => backend.embed_query(query),
        }
    }
}

struct HashingBackend {
    dimensions: usize,
}

impl HashingBackend {
    fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }

    fn embed_texts(&self, texts: &[String]) -> Vec<Vec<f32>> {
        texts.iter().map(|text| self.embed_text(text)).collect()
    }

    fn embed_text(&self, text: &str) -> Vec<f32> {
        let mut buckets = HashMap::<usize, f32>::new();
        for token in tokenize(text) {
            let index = (hash_u64(&(0u8, &token)) as usize) % self.dimensions;
            let sign = if hash_u64(&(1u8, &token)) & 1 == 0 {
                1.0f32
            } else {
                -1.0f32
            };
            *buckets.entry(index).or_insert(0.0) += sign;
        }

        let mut vector = vec![0.0f32; self.dimensions];
        for (index, value) in buckets {
            let magnitude = (1.0 + value.abs()).ln();
            vector[index] = value.signum() * magnitude;
        }
        normalize(&mut vector);
        vector
    }
}

#[cfg(feature = "fastembed-backend")]
struct FastembedBackend {
    model: Arc<TextEmbedding>,
}

#[cfg(feature = "fastembed-backend")]
impl FastembedBackend {
    /// 初始化 fastembed；首次运行会下载模型到缓存目录。
    fn new(config: &AppConfig) -> Result<Self> {
        let model_name = parse_fastembed_model(&config.fastembed_model)?;
        let options = InitOptions::new(model_name)
            .with_cache_dir(config.embedding_cache_dir.clone())
            .with_show_download_progress(true);
        let model = TextEmbedding::try_new(options)?;
        Ok(Self {
            model: Arc::new(model),
        })
    }

    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let payload = texts
            .iter()
            .map(|text| format!("passage: {text}"))
            .collect::<Vec<_>>();
        self.embed(&payload)
    }

    fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        let mut outputs = self.embed(&[format!("query: {query}")])?;
        outputs.pop().context("embedding model returned no vector")
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(self.model.embed(texts.to_vec(), None)?)
    }
}

#[cfg(feature = "fastembed-backend")]
fn parse_fastembed_model(name: &str) -> Result<EmbeddingModel> {
    let requested = name.trim();
    let requested_lower = requested.to_ascii_lowercase();

    for model_info in TextEmbedding::list_supported_models() {
        let variant_name = format!("{:?}", &model_info.model);
        if requested.eq_ignore_ascii_case(&variant_name)
            || requested_lower == model_info.model_code.to_ascii_lowercase()
        {
            return Ok(model_info.model);
        }
    }

    bail!(
        "unsupported fastembed model {}; supported values include MultilingualE5Small or intfloat/multilingual-e5-small",
        requested
    )
}

pub fn cosine_similarity(lhs: &[f32], rhs: &[f32]) -> f32 {
    if lhs.len() != rhs.len() || lhs.is_empty() || rhs.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut lhs_norm = 0.0f32;
    let mut rhs_norm = 0.0f32;

    for (&a, &b) in lhs.iter().zip(rhs.iter()) {
        dot += a * b;
        lhs_norm += a * a;
        rhs_norm += b * b;
    }

    if lhs_norm == 0.0 || rhs_norm == 0.0 {
        return 0.0;
    }

    dot / (lhs_norm.sqrt() * rhs_norm.sqrt())
}

fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut prev_cjk: Option<char> = None;

    for ch in text.chars() {
        if is_cjk(ch) {
            flush_word(&mut word, &mut tokens);
            tokens.push(ch.to_string());
            if let Some(prev) = prev_cjk {
                tokens.push(format!("{prev}{ch}"));
            }
            prev_cjk = Some(ch);
            continue;
        }

        if ch.is_alphanumeric() || ch == '_' {
            prev_cjk = None;
            for lower in ch.to_lowercase() {
                word.push(lower);
            }
            continue;
        }

        prev_cjk = None;
        flush_word(&mut word, &mut tokens);
    }

    flush_word(&mut word, &mut tokens);
    tokens
}

fn flush_word(word: &mut String, tokens: &mut Vec<String>) {
    if !word.is_empty() {
        tokens.push(std::mem::take(word));
    }
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm == 0.0 {
        return;
    }
    for value in vector {
        *value /= norm;
    }
}

fn hash_u64<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0xF900..=0xFAFF
            | 0x2F800..=0x2FA1F
    )
}

#[cfg(test)]
mod tests {
    use super::{EmbeddingEngine, cosine_similarity, tokenize};
    use crate::config::AppConfig;
    use tempfile::TempDir;

    #[test]
    fn tokenizer_handles_mixed_text() {
        let tokens = tokenize("EGO Planner 覆盖路径");
        assert!(tokens.iter().any(|token| token == "ego"));
        assert!(tokens.iter().any(|token| token == "planner"));
        assert!(tokens.iter().any(|token| token == "覆"));
        assert!(tokens.iter().any(|token| token == "覆盖"));
    }

    #[test]
    fn hashing_backend_is_deterministic() {
        let temp = TempDir::new().unwrap();
        let config = AppConfig {
            knowledge_root: temp.path().to_path_buf(),
            state_dir: temp.path().join("state"),
            database_path: temp.path().join("state/index.sqlite3"),
            embedding_backend: "hashing".to_string(),
            fastembed_model: "MultilingualE5Small".to_string(),
            embedding_cache_dir: temp.path().join("state/fastembed"),
            hashing_dimensions: 128,
            chunk_char_limit: 256,
            search_limit: 8,
            exclude_hidden: true,
            exclude_obsidian_dir: true,
            metadata_frontmatter_enabled: true,
            graph_enabled: true,
            graph_semantic_neighbors_per_node: 6,
            graph_semantic_min_score: 0.42,
        };
        let engine = EmbeddingEngine::new(&config).unwrap();
        let lhs = engine.embed_query("coverage planner").unwrap();
        let rhs = engine.embed_query("coverage planner").unwrap();
        assert_eq!(lhs, rhs);
    }

    #[test]
    fn cosine_similarity_handles_mismatch_and_zero_norm() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 0.0]), 0.0);
    }

    #[cfg(feature = "fastembed-backend")]
    #[test]
    fn fastembed_model_parser_accepts_variant_and_model_code() {
        let by_variant = super::parse_fastembed_model("MultilingualE5Small").unwrap();
        let by_model_code = super::parse_fastembed_model("intfloat/multilingual-e5-small").unwrap();

        assert_eq!(format!("{:?}", by_variant), "MultilingualE5Small");
        assert_eq!(format!("{:?}", by_model_code), "MultilingualE5Small");
    }
}
