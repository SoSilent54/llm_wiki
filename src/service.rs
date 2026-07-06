use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{
    config::AppConfig,
    db::IndexDatabase,
    embed::{EmbeddingEngine, cosine_similarity},
    indexer::Indexer,
    markdown,
    model::{
        DocumentListItem, DocumentOutlineResponse, GraphEdgeRecord, GraphNodeRecord,
        LibraryOverviewResponse, LineSpan, ListDocumentsResponse, MetadataCheckResponse,
        MetadataTemplateResponse, RelatedHit, RelatedResponse, SearchHit, SearchResponse,
        SectionOutlineItem, SectionRecord, SectionSearchHit, SectionSearchResponse, SourceAnchor,
    },
};

#[derive(Clone)]
pub struct KnowledgeService {
    config: AppConfig,
    db: IndexDatabase,
    embedder: EmbeddingEngine,
}

impl KnowledgeService {
    pub fn new(config: AppConfig) -> Result<Self> {
        let db = IndexDatabase::new(&config.database_path);
        db.init()?;
        let embedder = EmbeddingEngine::new(&config)?;
        Ok(Self {
            config,
            db,
            embedder,
        })
    }

    /// 仅用于不需要 embedding 的本地导航命令，避免无谓依赖 ORT 运行时。
    pub fn new_without_embedder(config: AppConfig) -> Result<Self> {
        let db = IndexDatabase::new(&config.database_path);
        db.init()?;

        let mut placeholder = config.clone();
        placeholder.embedding_backend = "hashing".to_string();
        if placeholder.hashing_dimensions == 0 {
            placeholder.hashing_dimensions = 1;
        }
        let embedder = EmbeddingEngine::new(&placeholder)?;

        Ok(Self {
            config,
            db,
            embedder,
        })
    }

    pub fn reindex_all(&self) -> Result<crate::model::IndexStats> {
        Indexer::new(&self.config, &self.db, &self.embedder).reindex_all()
    }

    /// 返回知识库分层概览，作为图谱/导航入口。
    pub fn library_overview(&self) -> Result<LibraryOverviewResponse> {
        Ok(LibraryOverviewResponse {
            doc_count: self.db.document_paths()?.len(),
            section_count: self.db.total_sections()?,
            chunk_count: self.db.total_chunks()?,
            top_dirs: self.db.load_top_dir_buckets(8)?,
            top_tags: self.db.load_top_tag_buckets(8)?,
        })
    }

    /// 列出知识库目录树中的文档/目录节点，便于 agent 缩小浏览范围。
    pub fn list_documents(
        &self,
        prefix: Option<&str>,
        depth: Option<usize>,
    ) -> Result<ListDocumentsResponse> {
        let prefix = normalize_prefix(prefix.unwrap_or_default())?;
        let depth = depth.unwrap_or(1);
        let nodes = self.db.load_all_graph_nodes()?;
        let edges = self.db.load_all_graph_edges()?;
        if nodes.is_empty() {
            bail!("graph is empty; run index first");
        }
        let node_map = nodes
            .iter()
            .cloned()
            .map(|node| (node.node_id.clone(), node))
            .collect::<HashMap<_, _>>();
        let child_counts = build_child_count_map(&edges, &node_map);

        let mut items = nodes
            .into_iter()
            .filter(|node| matches!(node.node_type.as_str(), "dir" | "doc"))
            .filter_map(|node| {
                let candidate_path = node.ref_path.clone();
                let relative_depth = relative_depth_under_prefix(&prefix, &candidate_path)?;
                let include = if relative_depth == 0 {
                    node.node_type == "doc" && candidate_path == prefix
                } else {
                    relative_depth <= depth
                };
                if !include {
                    return None;
                }

                let payload = parse_payload(&node.payload_json).ok()?;
                let title = if node.node_type == "doc" {
                    payload
                        .get("title")
                        .and_then(|value| value.as_str())
                        .map(ToOwned::to_owned)
                        .filter(|value| !value.is_empty())
                        .or_else(|| Some(node.label.clone()))
                } else {
                    None
                };
                let tag_sample = payload
                    .get("tags")
                    .and_then(|value| value.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                    .take(5)
                    .collect::<Vec<_>>();

                Some(DocumentListItem {
                    path: candidate_path,
                    node_type: node.node_type.clone(),
                    title,
                    child_count: *child_counts.get(&node.node_id).unwrap_or(&0),
                    tag_sample,
                })
            })
            .collect::<Vec<_>>();

        items.sort_by(|lhs, rhs| {
            node_type_sort_key(&lhs.node_type)
                .cmp(&node_type_sort_key(&rhs.node_type))
                .then_with(|| lhs.path.cmp(&rhs.path))
        });

        Ok(ListDocumentsResponse {
            prefix,
            depth,
            nodes: items,
        })
    }

    /// 从图谱层扩展相关节点；默认优先强边，再回落到语义边。
    pub fn related(
        &self,
        relative_path: &str,
        heading_path: Option<&str>,
        limit: Option<usize>,
        edge_types: Option<&[String]>,
    ) -> Result<RelatedResponse> {
        let _ = self.resolve_relative_path(relative_path)?;
        let nodes = self.db.load_all_graph_nodes()?;
        let edges = self.db.load_all_graph_edges()?;
        if nodes.is_empty() {
            bail!("graph is empty; run index first");
        }

        let node_map = nodes
            .iter()
            .cloned()
            .map(|node| (node.node_id.clone(), node))
            .collect::<HashMap<_, _>>();
        let source_node = nodes
            .iter()
            .find(|node| {
                if let Some(heading_path) = heading_path {
                    node.node_type == "section"
                        && node.ref_path == relative_path
                        && node.ref_section == heading_path
                } else {
                    node.node_type == "doc"
                        && node.ref_path == relative_path
                        && node.ref_section.is_empty()
                }
            })
            .with_context(|| {
                if let Some(heading_path) = heading_path {
                    format!(
                        "graph node not found for section {} :: {}; run index first",
                        relative_path, heading_path
                    )
                } else {
                    format!(
                        "graph node not found for document {}; run index first",
                        relative_path
                    )
                }
            })?;

        let requested_types = edge_types
            .map(|values| values.iter().cloned().collect::<HashSet<_>>())
            .unwrap_or_else(|| {
                default_related_edge_types()
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            });
        let limit = limit.unwrap_or(self.config.search_limit);

        let mut hits = edges
            .iter()
            .filter(|edge| edge.src_node_id == source_node.node_id)
            .filter(|edge| edge_type_selected(&edge.edge_type, &requested_types))
            .filter_map(|edge| build_related_hit(edge, &node_map).transpose())
            .collect::<Result<Vec<_>>>()?;

        hits.sort_by(|lhs, rhs| {
            related_edge_priority(&lhs.edge_type)
                .cmp(&related_edge_priority(&rhs.edge_type))
                .then_with(|| rhs.weight.total_cmp(&lhs.weight))
                .then_with(|| lhs.target_type.cmp(&rhs.target_type))
                .then_with(|| lhs.target_path.cmp(&rhs.target_path))
                .then_with(|| lhs.target_heading.cmp(&rhs.target_heading))
        });

        let mut dedup = HashSet::new();
        hits.retain(|hit| {
            let key = format!(
                "{}|{}|{}",
                hit.target_type,
                hit.target_path,
                hit.target_heading.as_deref().unwrap_or_default()
            );
            dedup.insert(key)
        });
        hits.truncate(limit);

        Ok(RelatedResponse {
            source_path: relative_path.to_string(),
            source_heading_path: heading_path.map(ToOwned::to_owned),
            hits,
        })
    }

    /// 向量搜索当前知识库中的全部块。
    pub fn search(&self, query: &str, limit: Option<usize>) -> Result<SearchResponse> {
        let query_embedding = self.embedder.embed_query(query)?;
        let chunks = self.db.load_all_chunks()?;
        let total_chunks = chunks.len();
        let limit = limit.unwrap_or(self.config.search_limit);

        let mut hits = chunks
            .into_iter()
            .map(|chunk| SearchHit {
                doc_path: chunk.doc_path.clone(),
                heading_path: chunk.heading_path.clone(),
                score: cosine_similarity(&query_embedding, &chunk.embedding),
                text: chunk.text.clone(),
                anchor: evidence_anchor(&chunk),
            })
            .collect::<Vec<_>>();

        hits.sort_by(|lhs, rhs| rhs.score.total_cmp(&lhs.score));
        hits.truncate(limit);

        Ok(SearchResponse {
            query: query.to_string(),
            total_chunks,
            hits,
        })
    }

    /// 向量搜索当前知识库中的全部 section 摘要层。
    pub fn search_sections(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<SectionSearchResponse> {
        let query_embedding = self.embedder.embed_query(query)?;
        let sections = self.db.load_all_sections()?;
        let section_map = sections
            .into_iter()
            .map(|section| (section.section_id.clone(), section))
            .collect::<HashMap<_, _>>();
        let section_embeddings = self.db.load_all_section_embeddings()?;
        let total_sections = section_embeddings.len();
        let limit = limit.unwrap_or(self.config.search_limit);

        let mut hits = section_embeddings
            .into_iter()
            .filter_map(|section| {
                let record = section_map.get(&section.section_id)?;
                Some(SectionSearchHit {
                    doc_path: section.doc_path,
                    heading_path: section.heading_path,
                    score: cosine_similarity(&query_embedding, &section.embedding),
                    first_paragraph: section.first_paragraph,
                    anchor: section_anchor(record),
                })
            })
            .collect::<Vec<_>>();

        hits.sort_by(|lhs, rhs| rhs.score.total_cmp(&lhs.score));
        hits.truncate(limit);

        Ok(SectionSearchResponse {
            query: query.to_string(),
            total_sections,
            hits,
        })
    }

    /// 返回指定文档的推断 metadata 模板，便于 agent 预览回写内容。
    pub fn metadata_template(&self, relative_path: &str) -> Result<MetadataTemplateResponse> {
        let path = self.resolve_relative_path(relative_path)?;
        markdown::metadata_template_for_document(&self.config.knowledge_root, &path)
    }

    /// 检查单个文档的 frontmatter 语法与 lint 规则。
    pub fn check_metadata(&self, relative_path: &str) -> Result<MetadataCheckResponse> {
        let path = self.resolve_relative_path(relative_path)?;
        markdown::check_metadata_document(&self.config.knowledge_root, &path)
    }

    /// 返回文档的 section 大纲，便于 agent 先看结构再精读。
    pub fn document_outline(&self, relative_path: &str) -> Result<DocumentOutlineResponse> {
        let path = self.resolve_relative_path(relative_path)?;
        let doc = markdown::load_document(
            &self.config.knowledge_root,
            &path,
            self.config.metadata_frontmatter_enabled,
        )?;
        let sections = self.db.load_sections_by_doc(relative_path)?;

        Ok(DocumentOutlineResponse {
            path: relative_path.to_string(),
            title: doc.metadata.as_ref().and_then(|metadata| {
                (!metadata.title.is_empty()).then_some(metadata.title.clone())
            }),
            sections: sections
                .into_iter()
                .map(|section| SectionOutlineItem {
                    ordinal: section.ordinal,
                    heading_path: section.heading_path.clone(),
                    heading_level: section.heading_level,
                    parent_heading_path: section.parent_heading_path.clone(),
                    first_paragraph: section.first_paragraph.clone(),
                    anchor: section_anchor(&section),
                })
                .collect(),
        })
    }

    fn resolve_relative_path(&self, relative_path: &str) -> Result<PathBuf> {
        validate_relative_reference(relative_path)?;
        let relative = Path::new(relative_path);
        let joined = self.config.knowledge_root.join(relative);
        let canonical = joined
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", joined.display()))?;
        let root = self.config.knowledge_root.canonicalize().with_context(|| {
            format!(
                "failed to resolve knowledge root {}",
                self.config.knowledge_root.display()
            )
        })?;
        if !canonical.starts_with(&root) {
            bail!("path escapes knowledge_root");
        }

        Ok(canonical)
    }
}

fn normalize_prefix(prefix: &str) -> Result<String> {
    let trimmed = prefix.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Ok(String::new());
    }
    validate_relative_reference(trimmed)?;
    Ok(trimmed.trim_matches('/').to_string())
}

fn validate_relative_reference(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute() {
        bail!("path must be relative to knowledge_root");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("path must not contain parent traversal");
    }
    Ok(())
}

fn relative_depth_under_prefix(prefix: &str, candidate: &str) -> Option<usize> {
    let normalized = candidate.trim_matches('/');
    if prefix.is_empty() {
        return Some(path_component_count(normalized));
    }
    if normalized == prefix {
        return Some(0);
    }
    let suffix = normalized.strip_prefix(&format!("{prefix}/"))?;
    Some(path_component_count(suffix))
}

fn path_component_count(path: &str) -> usize {
    if path.is_empty() || path == "." {
        0
    } else {
        path.split('/')
            .filter(|segment| !segment.is_empty())
            .count()
    }
}

fn build_child_count_map(
    edges: &[GraphEdgeRecord],
    node_map: &HashMap<String, GraphNodeRecord>,
) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for edge in edges {
        if edge.edge_type != "contains" {
            continue;
        }
        let Some(src) = node_map.get(&edge.src_node_id) else {
            continue;
        };
        let Some(dst) = node_map.get(&edge.dst_node_id) else {
            continue;
        };

        let should_count = match src.node_type.as_str() {
            "dir" => matches!(dst.node_type.as_str(), "dir" | "doc"),
            "doc" => dst.node_type == "section",
            _ => false,
        };
        if should_count {
            *counts.entry(src.node_id.clone()).or_insert(0) += 1;
        }
    }
    counts
}

fn node_type_sort_key(node_type: &str) -> usize {
    match node_type {
        "dir" => 0,
        "doc" => 1,
        _ => 2,
    }
}

fn default_related_edge_types() -> [&'static str; 4] {
    [
        "related_to",
        "links_to",
        "tagged_with",
        "semantic_similar_*",
    ]
}

fn edge_type_selected(edge_type: &str, requested_types: &HashSet<String>) -> bool {
    requested_types.iter().any(|candidate| {
        candidate == edge_type
            || (candidate == "semantic_similar_*" && edge_type.starts_with("semantic_similar_"))
    })
}

fn build_related_hit(
    edge: &GraphEdgeRecord,
    node_map: &HashMap<String, GraphNodeRecord>,
) -> Result<Option<RelatedHit>> {
    let Some(target) = node_map.get(&edge.dst_node_id) else {
        return Ok(None);
    };
    let payload = parse_payload(&target.payload_json)?;
    let target_path = if target.node_type == "tag" {
        target.label.clone()
    } else {
        target.ref_path.clone()
    };

    Ok(Some(RelatedHit {
        target_type: target.node_type.clone(),
        target_path,
        target_heading: (target.node_type == "section" && !target.ref_section.is_empty())
            .then_some(target.ref_section.clone()),
        edge_type: edge.edge_type.clone(),
        weight: edge.weight,
        why: explain_edge(edge)?,
        anchor: related_anchor(target, &payload),
    }))
}

fn explain_edge(edge: &GraphEdgeRecord) -> Result<String> {
    let payload = parse_payload(&edge.evidence_json)?;
    Ok(match edge.edge_type.as_str() {
        "related_to" => payload
            .get("source")
            .and_then(|value| value.as_str())
            .map(|source| format!("显式关联：{source}"))
            .unwrap_or_else(|| "显式关联".to_string()),
        "tagged_with" => payload
            .get("tag")
            .and_then(|value| value.as_str())
            .map(|tag| format!("共享 tag：{tag}"))
            .unwrap_or_else(|| "共享 tag".to_string()),
        "semantic_similar_doc" | "semantic_similar_section" => {
            format!("语义相似度 {:.3}", edge.weight)
        }
        "links_to" => "正文链接".to_string(),
        "contains" => "结构包含".to_string(),
        other => other.to_string(),
    })
}

fn section_anchor(section: &SectionRecord) -> SourceAnchor {
    SourceAnchor {
        kind: "section".to_string(),
        path: section.doc_path.clone(),
        span: Some(LineSpan {
            start_line: section.heading_line,
            end_line: section.end_line,
        }),
        heading_path: (!section.heading_path.is_empty()).then_some(section.heading_path.clone()),
        heading_level: Some(section.heading_level),
        section_ordinal: Some(section.ordinal),
        chunk_ordinal: None,
    }
}

fn evidence_anchor(chunk: &crate::model::ChunkRecord) -> SourceAnchor {
    SourceAnchor {
        kind: "evidence".to_string(),
        path: chunk.doc_path.clone(),
        span: Some(LineSpan {
            start_line: chunk.start_line,
            end_line: chunk.end_line,
        }),
        heading_path: (!chunk.heading_path.is_empty()).then_some(chunk.heading_path.clone()),
        heading_level: None,
        section_ordinal: None,
        chunk_ordinal: Some(chunk.chunk_ordinal_in_section),
    }
}

fn document_anchor(path: &str) -> SourceAnchor {
    SourceAnchor {
        kind: "document".to_string(),
        path: path.to_string(),
        span: None,
        heading_path: None,
        heading_level: None,
        section_ordinal: None,
        chunk_ordinal: None,
    }
}

fn related_anchor(target: &GraphNodeRecord, payload: &Value) -> Option<SourceAnchor> {
    match target.node_type.as_str() {
        "doc" => Some(document_anchor(&target.ref_path)),
        "section" => {
            let ordinal = payload.get("ordinal")?.as_i64()?;
            let heading_level = payload.get("heading_level")?.as_i64()?;
            let heading_line = payload.get("heading_line")?.as_u64()? as usize;
            let end_line = payload.get("end_line")?.as_u64()? as usize;
            Some(SourceAnchor {
                kind: "section".to_string(),
                path: target.ref_path.clone(),
                span: Some(LineSpan {
                    start_line: heading_line,
                    end_line,
                }),
                heading_path: (!target.ref_section.is_empty())
                    .then_some(target.ref_section.clone()),
                heading_level: Some(heading_level),
                section_ordinal: Some(ordinal),
                chunk_ordinal: None,
            })
        }
        _ => None,
    }
}
fn parse_payload(payload_json: &str) -> Result<Value> {
    serde_json::from_str(payload_json).map_err(Into::into)
}

fn related_edge_priority(edge_type: &str) -> usize {
    match edge_type {
        "related_to" => 0,
        "links_to" => 1,
        "tagged_with" => 2,
        "semantic_similar_doc" | "semantic_similar_section" => 3,
        _ => 4,
    }
}
