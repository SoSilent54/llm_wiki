use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub knowledge_root: PathBuf,
    pub state_dir: PathBuf,
    pub database_path: PathBuf,
    pub embedding_backend: String,
    pub fastembed_model: String,
    pub embedding_cache_dir: PathBuf,
    pub hashing_dimensions: usize,
    pub chunk_char_limit: usize,
    pub search_limit: usize,
    pub exclude_hidden: bool,
    pub exclude_obsidian_dir: bool,
    #[serde(default = "default_metadata_frontmatter_enabled")]
    pub metadata_frontmatter_enabled: bool,
    #[serde(default = "default_graph_enabled")]
    pub graph_enabled: bool,
    #[serde(default = "default_graph_semantic_neighbors_per_node")]
    pub graph_semantic_neighbors_per_node: usize,
    #[serde(default = "default_graph_semantic_min_score")]
    pub graph_semantic_min_score: f32,
}

impl AppConfig {
    /// 读取配置并把相对路径解析到配置文件目录。
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let mut config: Self = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

        config.knowledge_root = resolve_path(base_dir, &config.knowledge_root);
        config.state_dir = resolve_path(base_dir, &config.state_dir);
        config.database_path = resolve_path(base_dir, &config.database_path);
        config.embedding_cache_dir = resolve_path(base_dir, &config.embedding_cache_dir);

        if !config.knowledge_root.is_dir() {
            bail!(
                "knowledge_root does not exist or is not a directory: {}",
                config.knowledge_root.display()
            );
        }

        match config.embedding_backend.as_str() {
            "hashing" | "fastembed" => {}
            other => bail!("unsupported embedding_backend: {other}"),
        }
        if config.hashing_dimensions == 0 {
            bail!("hashing_dimensions must be greater than zero");
        }

        fs::create_dir_all(&config.state_dir).with_context(|| {
            format!("failed to create state_dir {}", config.state_dir.display())
        })?;
        if let Some(parent) = config.database_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create database dir {}", parent.display()))?;
        }
        fs::create_dir_all(&config.embedding_cache_dir).with_context(|| {
            format!(
                "failed to create embedding cache dir {}",
                config.embedding_cache_dir.display()
            )
        })?;

        Ok(config)
    }

    /// 生成会影响向量结果的配置指纹；切换后端或参数时必须触发重建。
    pub fn embedding_fingerprint(&self) -> String {
        match self.embedding_backend.as_str() {
            "hashing" => format!("hashing-v1|dim={}|prefix=none", self.hashing_dimensions),
            "fastembed" => format!(
                "fastembed-v1|model={}|prefix=query:passage",
                self.fastembed_model
            ),
            other => format!("unknown|backend={other}"),
        }
    }

    /// 生成图谱层版本指纹；结构边或规则变化时触发整图重建。
    pub fn graph_fingerprint(&self) -> String {
        format!(
            "graph-v2|enabled={}|neighbors={}|min_score={:.3}|edges=contains,tagged_with,related_to,semantic_similar_*",
            self.graph_enabled,
            self.graph_semantic_neighbors_per_node,
            self.graph_semantic_min_score
        )
    }
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn default_metadata_frontmatter_enabled() -> bool {
    true
}

fn default_graph_enabled() -> bool {
    true
}

fn default_graph_semantic_neighbors_per_node() -> usize {
    6
}

fn default_graph_semantic_min_score() -> f32 {
    0.42
}
