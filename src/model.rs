use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Markdown 文档的编译前模型；`raw_content` 保留原文，`content` 为去 frontmatter 后正文。
#[derive(Debug, Clone)]
pub struct MarkdownDocument {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub raw_content: String,
    pub content: String,
    pub metadata: Option<DocumentMetadata>,
    pub has_frontmatter: bool,
    pub metadata_parse_error: Option<String>,
    pub content_start_line: usize,
    pub frontmatter_span: Option<LineSpan>,
}

/// 原始 Markdown 文件中的 1-based 闭区间行范围。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct LineSpan {
    pub start_line: usize,
    pub end_line: usize,
}

/// 面向 agent 的源文件定位句柄；可直接映射到 `read(path:start-end)`。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SourceAnchor {
    pub kind: String,
    pub path: String,
    pub span: Option<LineSpan>,
    pub heading_path: Option<String>,
    pub heading_level: Option<i64>,
    pub section_ordinal: Option<i64>,
    pub chunk_ordinal: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct SectionDraft {
    pub heading_path: String,
    pub heading_level: i64,
    pub parent_heading_path: String,
    pub body_text: String,
    pub first_paragraph: String,
    pub heading_line: usize,
    pub body_start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone)]
pub struct ChunkDraft {
    pub section_ordinal: i64,
    pub chunk_ordinal_in_section: i64,
    pub heading_path: String,
    pub text: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone)]
pub struct SectionRecord {
    pub section_id: String,
    pub doc_path: String,
    pub ordinal: i64,
    pub heading_path: String,
    pub heading_level: i64,
    pub parent_heading_path: String,
    pub body_text: String,
    pub first_paragraph: String,
    pub section_hash: String,
    pub heading_line: usize,
    pub body_start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone)]
pub struct ChunkRecord {
    pub chunk_id: String,
    pub section_id: String,
    pub doc_path: String,
    pub ordinal: i64,
    pub chunk_ordinal_in_section: i64,
    pub heading_path: String,
    pub chunk_hash: String,
    pub text: String,
    pub start_line: usize,
    pub end_line: usize,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct DocumentManifest {
    pub file_hash: String,
    pub embedding_fingerprint: String,
}

/// 文件头 frontmatter 的规范化表示；缺字段时以空值表示，交由 lint 规则判断。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
pub struct DocumentMetadata {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default)]
    pub source_type: String,
    #[serde(default)]
    pub source_ref: String,
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_priority: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DocumentEmbeddingRecord {
    pub doc_path: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct SectionEmbeddingRecord {
    pub section_id: String,
    pub doc_path: String,
    pub heading_path: String,
    pub first_paragraph: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct GraphNodeRecord {
    pub node_id: String,
    pub node_type: String,
    pub ref_path: String,
    pub ref_section: String,
    pub label: String,
    pub payload_json: String,
}

#[derive(Debug, Clone)]
pub struct GraphEdgeRecord {
    pub edge_id: String,
    pub src_node_id: String,
    pub dst_node_id: String,
    pub edge_type: String,
    pub weight: f32,
    pub evidence_json: String,
    pub graph_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OverviewBucket {
    pub key: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LibraryOverviewResponse {
    pub doc_count: usize,
    pub section_count: usize,
    pub chunk_count: usize,
    pub top_dirs: Vec<OverviewBucket>,
    pub top_tags: Vec<OverviewBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DocumentListItem {
    pub path: String,
    pub node_type: String,
    pub title: Option<String>,
    pub child_count: usize,
    pub tag_sample: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListDocumentsResponse {
    pub prefix: String,
    pub depth: usize,
    pub nodes: Vec<DocumentListItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RelatedHit {
    pub target_type: String,
    pub target_path: String,
    pub target_heading: Option<String>,
    pub edge_type: String,
    pub weight: f32,
    pub why: String,
    pub anchor: Option<SourceAnchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RelatedResponse {
    pub source_path: String,
    pub source_heading_path: Option<String>,
    pub hits: Vec<RelatedHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    pub doc_path: String,
    pub heading_path: String,
    pub score: f32,
    pub text: String,
    pub anchor: SourceAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchResponse {
    pub query: String,
    pub total_chunks: usize,
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SectionSearchHit {
    pub doc_path: String,
    pub heading_path: String,
    pub score: f32,
    pub first_paragraph: String,
    pub anchor: SourceAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SectionSearchResponse {
    pub query: String,
    pub total_sections: usize,
    pub hits: Vec<SectionSearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SectionOutlineItem {
    pub ordinal: i64,
    pub heading_path: String,
    pub heading_level: i64,
    pub parent_heading_path: String,
    pub first_paragraph: String,
    pub anchor: SourceAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DocumentOutlineResponse {
    pub path: String,
    pub title: Option<String>,
    pub sections: Vec<SectionOutlineItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetadataLintIssue {
    pub severity: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetadataLintDocument {
    pub path: String,
    pub has_frontmatter: bool,
    pub metadata_valid: bool,
    pub issues: Vec<MetadataLintIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetadataLintReport {
    pub scanned_docs: usize,
    pub documents_with_frontmatter: usize,
    pub valid_docs: usize,
    pub error_count: usize,
    pub warning_count: usize,
    pub documents: Vec<MetadataLintDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetadataTemplateResponse {
    pub path: String,
    pub has_frontmatter: bool,
    pub metadata: DocumentMetadata,
    pub frontmatter: String,
    pub frontmatter_span: Option<LineSpan>,
    pub insert_before_line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetadataCheckResponse {
    pub path: String,
    pub has_frontmatter: bool,
    pub metadata_valid: bool,
    pub parse_error: Option<String>,
    pub metadata: Option<DocumentMetadata>,
    pub issues: Vec<MetadataLintIssue>,
    pub frontmatter_span: Option<LineSpan>,
    pub insert_before_line: usize,
    pub error_count: usize,
    pub warning_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IndexStats {
    pub scanned_docs: usize,
    pub updated_docs: usize,
    pub skipped_docs: usize,
    pub deleted_docs: usize,
    pub total_chunks: usize,
}
