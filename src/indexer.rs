use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path},
};

use anyhow::Result;
use serde_json::json;

use crate::{
    config::AppConfig,
    db::IndexDatabase,
    embed::{EmbeddingEngine, cosine_similarity},
    hash::sha256_hex,
    markdown,
    model::{
        ChunkRecord, DocumentEmbeddingRecord, GraphEdgeRecord, GraphNodeRecord, IndexStats,
        SectionEmbeddingRecord, SectionRecord,
    },
};

pub struct Indexer<'a> {
    config: &'a AppConfig,
    db: &'a IndexDatabase,
    embedder: &'a EmbeddingEngine,
}

#[derive(Debug, Clone)]
struct GraphDocumentInput {
    doc_path: String,
    title: String,
    tags: Vec<String>,
    related: Vec<String>,
    links: Vec<GraphLinkInput>,
    sections: Vec<SectionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphLinkInput {
    source_node_id: String,
    target_doc_path: String,
    href: String,
    text: String,
    line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InlineMarkdownLink {
    href: String,
    text: String,
}

impl<'a> Indexer<'a> {
    pub fn new(
        config: &'a AppConfig,
        db: &'a IndexDatabase,
        embedder: &'a EmbeddingEngine,
    ) -> Self {
        Self {
            config,
            db,
            embedder,
        }
    }

    /// 扫描全部 Markdown，并只更新发生变化的文档。
    pub fn reindex_all(&self) -> Result<IndexStats> {
        self.db.init()?;
        let files = markdown::discover_markdown_files(self.config)?;
        let known_manifests = self.db.document_manifests()?;
        let embedding_fingerprint = self.config.embedding_fingerprint();
        let mut seen_paths = HashSet::new();
        let mut graph_inputs = Vec::new();

        let mut stats = IndexStats {
            scanned_docs: files.len(),
            updated_docs: 0,
            skipped_docs: 0,
            deleted_docs: 0,
            total_chunks: 0,
        };

        for path in files {
            let doc = markdown::load_document(
                &self.config.knowledge_root,
                &path,
                self.config.metadata_frontmatter_enabled,
            )?;
            let file_hash = sha256_hex(doc.raw_content.as_bytes());
            let previous_manifest = known_manifests.get(&doc.relative_path);
            seen_paths.insert(doc.relative_path.clone());

            let section_drafts = markdown::build_sections(&doc);
            let sections = section_drafts
                .iter()
                .enumerate()
                .map(|(ordinal, section)| {
                    let section_hash = sha256_hex(section.body_text.as_bytes());
                    SectionRecord {
                        section_id: sha256_hex(format!(
                            "{}:{}:{}:{}",
                            doc.relative_path, ordinal, section.heading_path, section_hash
                        )),
                        doc_path: doc.relative_path.clone(),
                        ordinal: ordinal as i64,
                        heading_path: section.heading_path.clone(),
                        heading_level: section.heading_level,
                        parent_heading_path: section.parent_heading_path.clone(),
                        body_text: section.body_text.clone(),
                        first_paragraph: section.first_paragraph.clone(),
                        section_hash,
                        heading_line: section.heading_line,
                        body_start_line: section.body_start_line,
                        end_line: section.end_line,
                    }
                })
                .collect::<Vec<_>>();
            graph_inputs.push(build_graph_input(&doc, &sections));

            let chunk_drafts = markdown::chunk_document(&doc, self.config.chunk_char_limit);
            let expected_section_count = sections.len();
            let expected_chunk_count = chunk_drafts.len();
            let section_count = self.db.section_count_for_doc(&doc.relative_path)?;
            let has_doc_embedding = self.db.has_doc_embedding_for_doc(&doc.relative_path)?;
            let section_embedding_count = self
                .db
                .section_embedding_count_for_doc(&doc.relative_path)?;
            let chunk_count = self.db.chunk_count_for_doc(&doc.relative_path)?;
            let has_section_anchors = self.db.has_section_anchors_for_doc(&doc.relative_path)?;
            let has_chunk_anchors = self.db.has_chunk_anchors_for_doc(&doc.relative_path)?;
            let sections_ready = section_count == expected_section_count
                && (expected_section_count == 0 || has_section_anchors);
            let section_embeddings_ready = section_embedding_count == expected_section_count;
            let chunks_ready = chunk_count == expected_chunk_count
                && (expected_chunk_count == 0 || has_chunk_anchors);
            if sections_ready
                && has_doc_embedding
                && section_embeddings_ready
                && chunks_ready
                && previous_manifest.is_some_and(|manifest| {
                    manifest.file_hash == file_hash
                        && manifest.embedding_fingerprint == embedding_fingerprint
                })
            {
                stats.skipped_docs += 1;
                continue;
            }

            let doc_embedding_text = build_document_embedding_text(&doc, &sections);
            let doc_embedding = DocumentEmbeddingRecord {
                doc_path: doc.relative_path.clone(),
                embedding: self
                    .embedder
                    .embed_passages(&[doc_embedding_text])?
                    .into_iter()
                    .next()
                    .unwrap_or_default(),
            };

            let section_embedding_texts = sections
                .iter()
                .map(build_section_embedding_text)
                .collect::<Vec<_>>();
            let section_embedding_values = if section_embedding_texts.is_empty() {
                Vec::new()
            } else {
                self.embedder.embed_passages(&section_embedding_texts)?
            };
            let section_embeddings = sections
                .iter()
                .zip(section_embedding_values.into_iter())
                .map(|(section, embedding)| SectionEmbeddingRecord {
                    section_id: section.section_id.clone(),
                    doc_path: section.doc_path.clone(),
                    heading_path: section.heading_path.clone(),
                    first_paragraph: section.first_paragraph.clone(),
                    embedding,
                })
                .collect::<Vec<_>>();

            let chunk_embeddings = if chunk_drafts.is_empty() {
                Vec::new()
            } else {
                self.embedder.embed_passages(
                    &chunk_drafts
                        .iter()
                        .map(|chunk| chunk.text.clone())
                        .collect::<Vec<_>>(),
                )?
            };

            let chunks = chunk_drafts
                .into_iter()
                .zip(chunk_embeddings.into_iter())
                .enumerate()
                .map(|(ordinal, (chunk, embedding))| {
                    let chunk_hash = sha256_hex(chunk.text.as_bytes());
                    let section_id = sections
                        .get(chunk.section_ordinal as usize)
                        .map(|section| section.section_id.clone())
                        .unwrap_or_default();
                    ChunkRecord {
                        chunk_id: sha256_hex(format!(
                            "{}:{}:{}:{}",
                            doc.relative_path, ordinal, chunk.heading_path, chunk_hash
                        )),
                        section_id,
                        doc_path: doc.relative_path.clone(),
                        ordinal: ordinal as i64,
                        chunk_ordinal_in_section: chunk.chunk_ordinal_in_section,
                        heading_path: chunk.heading_path,
                        chunk_hash,
                        text: chunk.text,
                        start_line: chunk.start_line,
                        end_line: chunk.end_line,
                        embedding,
                    }
                })
                .collect::<Vec<_>>();

            self.db.replace_document(
                &doc.relative_path,
                &file_hash,
                &embedding_fingerprint,
                &doc_embedding,
                &sections,
                &section_embeddings,
                &chunks,
            )?;
            stats.updated_docs += 1;
        }

        let deleted_paths = self
            .db
            .document_paths()?
            .difference(&seen_paths)
            .cloned()
            .collect::<Vec<_>>();
        stats.deleted_docs = self.db.delete_documents(&deleted_paths)?;

        let desired_graph_fingerprint = self.config.graph_fingerprint();
        let has_graph = self.db.has_graph()?;
        let current_graph_fingerprint = self.db.current_graph_fingerprint()?;
        let graph_dirty = stats.updated_docs > 0
            || stats.deleted_docs > 0
            || if self.config.graph_enabled {
                if seen_paths.is_empty() {
                    has_graph
                } else {
                    !has_graph
                        || current_graph_fingerprint.as_deref()
                            != Some(desired_graph_fingerprint.as_str())
                }
            } else {
                has_graph
            };

        if graph_dirty {
            if self.config.graph_enabled {
                let doc_embeddings = self.db.load_all_doc_embeddings()?;
                let section_embeddings = self.db.load_all_section_embeddings()?;
                let (graph_nodes, graph_edges) = build_graph_records(
                    &graph_inputs,
                    &doc_embeddings,
                    &section_embeddings,
                    &desired_graph_fingerprint,
                    self.config.graph_semantic_neighbors_per_node,
                    self.config.graph_semantic_min_score,
                )?;
                self.db.replace_graph(&graph_nodes, &graph_edges)?;
            } else {
                self.db.replace_graph(&[], &[])?;
            }
        }

        stats.total_chunks = self.db.total_chunks()?;
        Ok(stats)
    }
}

fn build_document_embedding_text(
    doc: &crate::model::MarkdownDocument,
    sections: &[SectionRecord],
) -> String {
    let mut lines = vec![format!("Path: {}", doc.relative_path)];
    if let Some(metadata) = &doc.metadata {
        if !metadata.title.is_empty() {
            lines.push(format!("Title: {}", metadata.title));
        }
        if !metadata.tags.is_empty() {
            lines.push(format!("Tags: {}", metadata.tags.join(", ")));
        }
    }
    let heading_paths = sections
        .iter()
        .filter_map(|section| {
            (!section.heading_path.is_empty()).then_some(section.heading_path.as_str())
        })
        .take(12)
        .collect::<Vec<_>>();
    if !heading_paths.is_empty() {
        lines.push(format!("Headings: {}", heading_paths.join(" | ")));
    }
    lines.push(doc.content.trim().to_string());
    lines.join("\n\n")
}

fn build_section_embedding_text(section: &SectionRecord) -> String {
    let mut text = format!("Path: {}\n", section.doc_path);
    if !section.heading_path.is_empty() {
        text.push_str(&format!("Heading: {}\n\n", section.heading_path));
    } else {
        text.push('\n');
    }
    text.push_str(section.body_text.trim());
    text
}

fn build_graph_input(
    doc: &crate::model::MarkdownDocument,
    sections: &[SectionRecord],
) -> GraphDocumentInput {
    let title = doc
        .metadata
        .as_ref()
        .and_then(|metadata| (!metadata.title.is_empty()).then_some(metadata.title.clone()))
        .unwrap_or_else(|| fallback_doc_label(&doc.relative_path));
    let tags = doc
        .metadata
        .as_ref()
        .map(|metadata| metadata.tags.clone())
        .unwrap_or_default();
    let related = doc
        .metadata
        .as_ref()
        .map(|metadata| metadata.related.clone())
        .unwrap_or_default();
    let links = extract_graph_links(doc, sections);

    GraphDocumentInput {
        doc_path: doc.relative_path.clone(),
        title,
        tags,
        related,
        links,
        sections: sections.to_vec(),
    }
}

fn build_graph_records(
    docs: &[GraphDocumentInput],
    doc_embeddings: &[DocumentEmbeddingRecord],
    section_embeddings: &[SectionEmbeddingRecord],
    graph_fingerprint: &str,
    semantic_neighbors_per_node: usize,
    semantic_min_score: f32,
) -> Result<(Vec<GraphNodeRecord>, Vec<GraphEdgeRecord>)> {
    let mut dir_doc_counts = HashMap::<String, usize>::new();
    let mut tag_doc_counts = HashMap::<String, usize>::new();
    let all_doc_paths = docs
        .iter()
        .map(|doc| doc.doc_path.clone())
        .collect::<HashSet<_>>();

    for doc in docs {
        for dir in doc_dir_chain(&doc.doc_path) {
            *dir_doc_counts.entry(dir).or_insert(0) += 1;
        }
        for tag in &doc.tags {
            *tag_doc_counts.entry(tag.clone()).or_insert(0) += 1;
        }
    }

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut node_ids = HashSet::<String>::new();
    let mut edge_ids = HashSet::<String>::new();

    for (dir, doc_count) in &dir_doc_counts {
        push_node(
            &mut nodes,
            &mut node_ids,
            GraphNodeRecord {
                node_id: dir_node_id(dir),
                node_type: "dir".to_string(),
                ref_path: dir.clone(),
                ref_section: String::new(),
                label: dir_label(dir),
                payload_json: serde_json::to_string(&json!({
                    "doc_count": doc_count,
                    "depth": dir_depth(dir),
                }))?,
            },
        );
    }

    for (tag, doc_count) in &tag_doc_counts {
        push_node(
            &mut nodes,
            &mut node_ids,
            GraphNodeRecord {
                node_id: tag_node_id(tag),
                node_type: "tag".to_string(),
                ref_path: tag.clone(),
                ref_section: String::new(),
                label: tag.clone(),
                payload_json: serde_json::to_string(&json!({
                    "doc_count": doc_count,
                }))?,
            },
        );
    }

    for doc in docs {
        let doc_node = doc_node_id(&doc.doc_path);
        push_node(
            &mut nodes,
            &mut node_ids,
            GraphNodeRecord {
                node_id: doc_node.clone(),
                node_type: "doc".to_string(),
                ref_path: doc.doc_path.clone(),
                ref_section: String::new(),
                label: doc.title.clone(),
                payload_json: serde_json::to_string(&json!({
                    "title": doc.title,
                    "section_count": doc.sections.len(),
                    "tag_count": doc.tags.len(),
                    "tags": doc.tags,
                }))?,
            },
        );

        let dir_chain = doc_dir_chain(&doc.doc_path);
        for pair in dir_chain.windows(2) {
            push_edge(
                &mut edges,
                &mut edge_ids,
                graph_edge_record(
                    dir_node_id(&pair[0]),
                    dir_node_id(&pair[1]),
                    "contains",
                    1.0,
                    json!({"kind": "dir-child"}),
                    graph_fingerprint,
                )?,
            );
        }
        if let Some(parent_dir) = dir_chain.last() {
            push_edge(
                &mut edges,
                &mut edge_ids,
                graph_edge_record(
                    dir_node_id(parent_dir),
                    doc_node.clone(),
                    "contains",
                    1.0,
                    json!({"kind": "doc-child"}),
                    graph_fingerprint,
                )?,
            );
        }

        for section in &doc.sections {
            let section_node_id = section_node_id(section);
            push_node(
                &mut nodes,
                &mut node_ids,
                GraphNodeRecord {
                    node_id: section_node_id.clone(),
                    node_type: "section".to_string(),
                    ref_path: section.doc_path.clone(),
                    ref_section: section.heading_path.clone(),
                    label: section_label(section, &doc.title),
                    payload_json: serde_json::to_string(&json!({
                        "ordinal": section.ordinal,
                        "heading_level": section.heading_level,
                        "heading_line": section.heading_line,
                        "body_start_line": section.body_start_line,
                        "end_line": section.end_line,
                    }))?,
                },
            );
            push_edge(
                &mut edges,
                &mut edge_ids,
                graph_edge_record(
                    doc_node.clone(),
                    section_node_id,
                    "contains",
                    1.0,
                    json!({"ordinal": section.ordinal}),
                    graph_fingerprint,
                )?,
            );
        }

        for tag in &doc.tags {
            push_edge(
                &mut edges,
                &mut edge_ids,
                graph_edge_record(
                    doc_node.clone(),
                    tag_node_id(tag),
                    "tagged_with",
                    0.9,
                    json!({"tag": tag}),
                    graph_fingerprint,
                )?,
            );
        }

        for related in &doc.related {
            if related == &doc.doc_path || !all_doc_paths.contains(related) {
                continue;
            }
            push_edge(
                &mut edges,
                &mut edge_ids,
                graph_edge_record(
                    doc_node.clone(),
                    doc_node_id(related),
                    "related_to",
                    0.95,
                    json!({"source": "frontmatter.related"}),
                    graph_fingerprint,
                )?,
            );
        }

        let mut aggregated_links = HashMap::<String, (usize, &GraphLinkInput)>::new();
        for link in &doc.links {
            if link.target_doc_path == doc.doc_path
                || !all_doc_paths.contains(&link.target_doc_path)
            {
                continue;
            }
            push_edge(
                &mut edges,
                &mut edge_ids,
                graph_edge_record(
                    link.source_node_id.clone(),
                    doc_node_id(&link.target_doc_path),
                    "links_to",
                    0.94,
                    json!({
                        "href": link.href,
                        "text": link.text,
                        "line": link.line,
                        "mention_count": 1,
                        "target_path": link.target_doc_path,
                    }),
                    graph_fingerprint,
                )?,
            );
            aggregated_links
                .entry(link.target_doc_path.clone())
                .and_modify(|(count, _)| *count += 1)
                .or_insert((1, link));
        }
        for (target_doc_path, (mention_count, first_link)) in aggregated_links {
            push_edge(
                &mut edges,
                &mut edge_ids,
                graph_edge_record(
                    doc_node.clone(),
                    doc_node_id(&target_doc_path),
                    "links_to",
                    0.94,
                    json!({
                        "href": first_link.href,
                        "text": first_link.text,
                        "line": first_link.line,
                        "mention_count": mention_count,
                        "target_path": target_doc_path,
                    }),
                    graph_fingerprint,
                )?,
            );
        }
    }

    append_semantic_doc_edges(
        &mut edges,
        &mut edge_ids,
        docs,
        doc_embeddings,
        graph_fingerprint,
        semantic_neighbors_per_node,
        semantic_min_score,
    )?;
    append_semantic_section_edges(
        &mut edges,
        &mut edge_ids,
        docs,
        section_embeddings,
        graph_fingerprint,
        semantic_neighbors_per_node,
        semantic_min_score,
    )?;

    Ok((nodes, edges))
}

fn append_semantic_doc_edges(
    edges: &mut Vec<GraphEdgeRecord>,
    edge_ids: &mut HashSet<String>,
    docs: &[GraphDocumentInput],
    doc_embeddings: &[DocumentEmbeddingRecord],
    graph_fingerprint: &str,
    semantic_neighbors_per_node: usize,
    semantic_min_score: f32,
) -> Result<()> {
    if semantic_neighbors_per_node == 0 {
        return Ok(());
    }

    let known_docs = docs
        .iter()
        .map(|doc| (doc.doc_path.as_str(), top_dir_for_doc_path(&doc.doc_path)))
        .collect::<HashMap<_, _>>();

    for src in doc_embeddings {
        let Some(src_top_dir) = known_docs.get(src.doc_path.as_str()) else {
            continue;
        };
        let mut candidates: Vec<(String, f32, f32, bool)> = Vec::new();
        for dst in doc_embeddings {
            if src.doc_path == dst.doc_path {
                continue;
            }
            let Some(dst_top_dir) = known_docs.get(dst.doc_path.as_str()) else {
                continue;
            };
            let raw_score = cosine_similarity(&src.embedding, &dst.embedding);
            let same_top_dir = src_top_dir == dst_top_dir;
            let adjusted_score =
                adjusted_semantic_score(raw_score, same_top_dir, semantic_min_score);
            if adjusted_score >= semantic_min_score {
                candidates.push((
                    dst.doc_path.clone(),
                    adjusted_score,
                    raw_score,
                    same_top_dir,
                ));
            }
        }

        candidates.sort_by(|lhs, rhs| rhs.1.total_cmp(&lhs.1));
        candidates.truncate(semantic_neighbors_per_node);
        for (dst_path, adjusted_score, raw_score, same_top_dir) in candidates {
            push_edge(
                edges,
                edge_ids,
                graph_edge_record(
                    doc_node_id(&src.doc_path),
                    doc_node_id(&dst_path),
                    "semantic_similar_doc",
                    adjusted_score,
                    json!({
                        "raw_score": raw_score,
                        "adjusted_score": adjusted_score,
                        "same_top_dir": same_top_dir,
                    }),
                    graph_fingerprint,
                )?,
            );
        }
    }

    Ok(())
}

fn append_semantic_section_edges(
    edges: &mut Vec<GraphEdgeRecord>,
    edge_ids: &mut HashSet<String>,
    docs: &[GraphDocumentInput],
    section_embeddings: &[SectionEmbeddingRecord],
    graph_fingerprint: &str,
    semantic_neighbors_per_node: usize,
    semantic_min_score: f32,
) -> Result<()> {
    if semantic_neighbors_per_node == 0 {
        return Ok(());
    }

    let known_sections = docs
        .iter()
        .flat_map(|doc| {
            doc.sections.iter().map(|section| {
                (
                    section.section_id.as_str(),
                    top_dir_for_doc_path(&section.doc_path),
                )
            })
        })
        .collect::<HashMap<_, _>>();

    for src in section_embeddings {
        let Some(src_top_dir) = known_sections.get(src.section_id.as_str()) else {
            continue;
        };
        let mut candidates: Vec<(String, f32, f32, bool)> = Vec::new();
        for dst in section_embeddings {
            if src.section_id == dst.section_id {
                continue;
            }
            let Some(dst_top_dir) = known_sections.get(dst.section_id.as_str()) else {
                continue;
            };
            let raw_score = cosine_similarity(&src.embedding, &dst.embedding);
            let same_top_dir = src_top_dir == dst_top_dir;
            let adjusted_score =
                adjusted_semantic_score(raw_score, same_top_dir, semantic_min_score);
            if adjusted_score >= semantic_min_score {
                candidates.push((
                    dst.section_id.clone(),
                    adjusted_score,
                    raw_score,
                    same_top_dir,
                ));
            }
        }

        candidates.sort_by(|lhs, rhs| rhs.1.total_cmp(&lhs.1));
        candidates.truncate(semantic_neighbors_per_node);
        for (dst_section_id, adjusted_score, raw_score, same_top_dir) in candidates {
            push_edge(
                edges,
                edge_ids,
                graph_edge_record(
                    format!("section::{}", src.section_id),
                    format!("section::{}", dst_section_id),
                    "semantic_similar_section",
                    adjusted_score,
                    json!({
                        "raw_score": raw_score,
                        "adjusted_score": adjusted_score,
                        "same_top_dir": same_top_dir,
                    }),
                    graph_fingerprint,
                )?,
            );
        }
    }

    Ok(())
}

fn adjusted_semantic_score(raw_score: f32, same_top_dir: bool, semantic_min_score: f32) -> f32 {
    if same_top_dir && raw_score < semantic_min_score {
        (raw_score + 0.03).min(0.89)
    } else {
        raw_score
    }
}

fn graph_edge_record(
    src_node_id: String,
    dst_node_id: String,
    edge_type: &str,
    weight: f32,
    evidence: serde_json::Value,
    graph_fingerprint: &str,
) -> Result<GraphEdgeRecord> {
    Ok(GraphEdgeRecord {
        edge_id: sha256_hex(format!("{src_node_id}|{dst_node_id}|{edge_type}")),
        src_node_id,
        dst_node_id,
        edge_type: edge_type.to_string(),
        weight,
        evidence_json: serde_json::to_string(&evidence)?,
        graph_fingerprint: graph_fingerprint.to_string(),
    })
}

fn push_node(nodes: &mut Vec<GraphNodeRecord>, seen: &mut HashSet<String>, node: GraphNodeRecord) {
    if seen.insert(node.node_id.clone()) {
        nodes.push(node);
    }
}

fn push_edge(edges: &mut Vec<GraphEdgeRecord>, seen: &mut HashSet<String>, edge: GraphEdgeRecord) {
    if seen.insert(edge.edge_id.clone()) {
        edges.push(edge);
    }
}

fn extract_graph_links(
    doc: &crate::model::MarkdownDocument,
    sections: &[SectionRecord],
) -> Vec<GraphLinkInput> {
    let mut links = Vec::new();
    for section in sections {
        for (line_offset, line) in section.body_text.lines().enumerate() {
            let line_number = section.body_start_line + line_offset;
            for inline_link in extract_inline_links_from_line(line) {
                let Some(target_doc_path) =
                    normalize_markdown_doc_link_target(&doc.relative_path, &inline_link.href)
                else {
                    continue;
                };
                links.push(GraphLinkInput {
                    source_node_id: section_node_id(section),
                    target_doc_path,
                    href: inline_link.href,
                    text: inline_link.text,
                    line: line_number,
                });
            }
        }
    }
    links
}

fn extract_inline_links_from_line(line: &str) -> Vec<InlineMarkdownLink> {
    let bytes = line.as_bytes();
    let mut cursor = 0usize;
    let mut links = Vec::new();

    while cursor < bytes.len() {
        if bytes[cursor] != b'[' || (cursor > 0 && bytes[cursor - 1] == b'!') {
            cursor += 1;
            continue;
        }

        let Some(text_end_rel) = line[cursor + 1..].find(']') else {
            break;
        };
        let text_end = cursor + 1 + text_end_rel;
        let mut target_start = text_end + 1;
        while target_start < bytes.len() && bytes[target_start].is_ascii_whitespace() {
            target_start += 1;
        }
        if target_start >= bytes.len() || bytes[target_start] != b'(' {
            cursor += 1;
            continue;
        }

        let Some(target_end_rel) = line[target_start + 1..].find(')') else {
            break;
        };
        let target_end = target_start + 1 + target_end_rel;
        let Some(href) = extract_inline_link_href(&line[target_start + 1..target_end]) else {
            cursor = target_end + 1;
            continue;
        };

        links.push(InlineMarkdownLink {
            href,
            text: line[cursor + 1..text_end].trim().to_string(),
        });
        cursor = target_end + 1;
    }

    links
}

fn extract_inline_link_href(raw_target: &str) -> Option<String> {
    let trimmed = raw_target.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix('<') {
        let end = rest.find('>')?;
        let href = rest[..end].trim();
        return (!href.is_empty()).then(|| href.to_string());
    }

    trimmed
        .split_whitespace()
        .next()
        .filter(|href| !href.is_empty())
        .map(str::to_string)
}

fn normalize_markdown_doc_link_target(source_doc_path: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty()
        || href.starts_with('#')
        || href.contains('#')
        || href.contains('?')
        || href.starts_with('/')
        || href
            .split('/')
            .next()
            .is_some_and(|segment| segment.contains(':'))
    {
        return None;
    }

    if !href.to_ascii_lowercase().ends_with(".md") {
        return None;
    }

    let mut normalized = Vec::<String>::new();
    if let Some(parent) = Path::new(source_doc_path).parent() {
        for component in parent.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(value) => normalized.push(value.to_string_lossy().into_owned()),
                _ => return None,
            }
        }
    }

    for component in Path::new(href).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value.to_string_lossy().into_owned()),
            Component::ParentDir => {
                normalized.pop()?;
            }
            _ => return None,
        }
    }

    let path = normalized.join("/");
    (!path.is_empty()).then_some(path)
}

fn fallback_doc_label(doc_path: &str) -> String {
    Path::new(doc_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(doc_path)
        .to_string()
}

fn doc_dir_chain(doc_path: &str) -> Vec<String> {
    let path = Path::new(doc_path);
    let mut chain = vec![".".to_string()];
    let mut current = Path::new("").to_path_buf();
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            current.push(component.as_os_str());
            let entry = current.to_string_lossy().replace('\\', "/");
            if !entry.is_empty() {
                chain.push(entry);
            }
        }
    }
    chain
}

fn top_dir_for_doc_path(doc_path: &str) -> String {
    doc_path
        .split('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(".")
        .to_string()
}
fn dir_node_id(dir: &str) -> String {
    format!("dir::{dir}")
}

fn doc_node_id(doc_path: &str) -> String {
    format!("doc::{doc_path}")
}

fn tag_node_id(tag: &str) -> String {
    format!("tag::{tag}")
}

fn section_node_id(section: &SectionRecord) -> String {
    format!("section::{}", section.section_id)
}

fn dir_label(dir: &str) -> String {
    if dir == "." {
        return ".".to_string();
    }
    Path::new(dir)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(dir)
        .to_string()
}

fn dir_depth(dir: &str) -> usize {
    if dir == "." || dir.is_empty() {
        0
    } else {
        dir.split('/').count()
    }
}

fn section_label(section: &SectionRecord, doc_title: &str) -> String {
    if section.heading_path.is_empty() {
        doc_title.to_string()
    } else {
        section
            .heading_path
            .rsplit(" > ")
            .next()
            .unwrap_or(&section.heading_path)
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{build_graph_input, build_graph_records, doc_node_id, section_node_id};
    use crate::{
        hash::sha256_hex,
        markdown,
        model::{MarkdownDocument, SectionRecord},
    };
    use serde_json::Value;
    use std::path::PathBuf;

    fn test_doc(relative_path: &str, content: &str) -> MarkdownDocument {
        MarkdownDocument {
            relative_path: relative_path.to_string(),
            absolute_path: PathBuf::from(relative_path),
            raw_content: content.to_string(),
            content: content.to_string(),
            metadata: None,
            has_frontmatter: false,
            metadata_parse_error: None,
            content_start_line: 1,
            frontmatter_span: None,
        }
    }

    fn test_sections(doc: &MarkdownDocument) -> Vec<SectionRecord> {
        markdown::build_sections(doc)
            .into_iter()
            .enumerate()
            .map(|(ordinal, section)| SectionRecord {
                section_id: sha256_hex(format!(
                    "{}:{ordinal}:{}",
                    doc.relative_path, section.heading_path
                )),
                doc_path: doc.relative_path.clone(),
                ordinal: ordinal as i64,
                heading_path: section.heading_path,
                heading_level: section.heading_level,
                parent_heading_path: section.parent_heading_path,
                body_text: section.body_text,
                first_paragraph: section.first_paragraph,
                section_hash: format!("section-hash-{ordinal}"),
                heading_line: section.heading_line,
                body_start_line: section.body_start_line,
                end_line: section.end_line,
            })
            .collect()
    }

    #[test]
    fn build_graph_input_extracts_only_relative_markdown_links() {
        let doc = test_doc(
            "Research/GCOPTER/Ego-Planner.md",
            "# Planner\nSee [MINCO](../MINCO.md)\n![img](../skip.md)\n[anchor](#local)\n[external](https://example.com/demo.md)\n[fragment](../DDR-Opt.md#sec)\nSee [DDR](../DDR-Opt.md)\n",
        );
        let sections = test_sections(&doc);

        let graph_input = build_graph_input(&doc, &sections);

        assert_eq!(graph_input.links.len(), 2);
        assert_eq!(graph_input.links[0].target_doc_path, "Research/MINCO.md");
        assert_eq!(graph_input.links[0].href, "../MINCO.md");
        assert_eq!(graph_input.links[0].text, "MINCO");
        assert_eq!(graph_input.links[0].line, 2);
        assert_eq!(graph_input.links[1].target_doc_path, "Research/DDR-Opt.md");
        assert_eq!(graph_input.links[1].href, "../DDR-Opt.md");
        assert_eq!(graph_input.links[1].text, "DDR");
        assert_eq!(graph_input.links[1].line, 7);
    }

    #[test]
    fn build_graph_records_emits_section_and_doc_level_links_to_edges() {
        let source_doc = test_doc(
            "Research/GCOPTER/Ego-Planner.md",
            "# Planner\nSee [MINCO](../MINCO.md)\n## More\nAgain [MINCO](../MINCO.md)\n",
        );
        let target_doc = test_doc("Research/MINCO.md", "# MINCO\nTarget\n");
        let source_sections = test_sections(&source_doc);
        let target_sections = test_sections(&target_doc);
        let docs = vec![
            build_graph_input(&source_doc, &source_sections),
            build_graph_input(&target_doc, &target_sections),
        ];

        let (_nodes, edges) =
            build_graph_records(&docs, &[], &[], "graph-v-test", 0, 0.42).unwrap();
        let section_edges = edges
            .iter()
            .filter(|edge| {
                edge.edge_type == "links_to"
                    && edge.src_node_id.starts_with("section::")
                    && edge.dst_node_id == doc_node_id("Research/MINCO.md")
            })
            .collect::<Vec<_>>();
        let doc_edge = edges
            .iter()
            .find(|edge| {
                edge.edge_type == "links_to"
                    && edge.src_node_id == doc_node_id("Research/GCOPTER/Ego-Planner.md")
                    && edge.dst_node_id == doc_node_id("Research/MINCO.md")
            })
            .unwrap();

        assert_eq!(section_edges.len(), 2);
        assert!(
            section_edges
                .iter()
                .any(|edge| edge.src_node_id == section_node_id(&source_sections[0]))
        );
        assert!(
            section_edges
                .iter()
                .any(|edge| edge.src_node_id == section_node_id(&source_sections[1]))
        );
        assert_eq!(doc_edge.weight, 0.94);

        let evidence: Value = serde_json::from_str(&doc_edge.evidence_json).unwrap();
        assert_eq!(
            evidence.get("href").and_then(Value::as_str),
            Some("../MINCO.md")
        );
        assert_eq!(evidence.get("text").and_then(Value::as_str), Some("MINCO"));
        assert_eq!(evidence.get("line").and_then(Value::as_u64), Some(2));
        assert_eq!(
            evidence.get("mention_count").and_then(Value::as_u64),
            Some(2)
        );
        assert_eq!(
            evidence.get("target_path").and_then(Value::as_str),
            Some("Research/MINCO.md")
        );
    }
}
