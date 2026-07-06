use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use walkdir::{DirEntry, WalkDir};

use crate::{
    config::AppConfig,
    model::{
        ChunkDraft, DocumentMetadata, LineSpan, MarkdownDocument, MetadataCheckResponse,
        MetadataLintDocument, MetadataLintIssue, MetadataLintReport, MetadataTemplateResponse,
        SectionDraft,
    },
};

const SOURCE_TYPES: &[&str] = &["note", "paper", "blog", "repo", "api", "manual", "concept"];
const STATUS_VALUES: &[&str] = &["seed", "draft", "stable", "archived"];
const REVIEW_PRIORITIES: &[&str] = &["low", "medium", "high"];
const KNOWN_TAG_DOMAINS: &[&str] = &["planning", "control", "cpp", "estimation", "math", "system"];

const METADATA_TEMPLATE: &str = r#"---
title: Example Title
tags:
  - planning/trajectory-optimization
aliases: []
related: []
source_type: note
source_ref: local://path-or-url
status: draft
domain: planning
keywords: []
updated_by: agent
updated_at: 2026-07-06T00:00:00Z
review_priority: medium
---"#;

/// 递归发现知识库中的 Markdown 文件。
pub fn discover_markdown_files(config: &AppConfig) -> Result<Vec<PathBuf>> {
    let exclude_hidden = config.exclude_hidden;
    let exclude_obsidian_dir = config.exclude_obsidian_dir;
    let root = config.knowledge_root.clone();

    let mut files = WalkDir::new(&root)
        .into_iter()
        .filter_entry(move |entry| should_descend(entry, exclude_hidden, exclude_obsidian_dir))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| is_markdown_file(path))
        .collect::<Vec<_>>();

    files.sort();
    Ok(files)
}

/// 返回统一 frontmatter 模板，便于 agent 或人工批量补全。
pub fn metadata_template() -> &'static str {
    METADATA_TEMPLATE
}

/// 扫描整个知识库并输出 metadata lint 报告。
pub fn lint_metadata_tree(config: &AppConfig) -> Result<MetadataLintReport> {
    let files = discover_markdown_files(config)?;
    let mut report = MetadataLintReport {
        scanned_docs: files.len(),
        documents_with_frontmatter: 0,
        valid_docs: 0,
        error_count: 0,
        warning_count: 0,
        documents: Vec::with_capacity(files.len()),
    };

    for path in files {
        let doc = load_document(&config.knowledge_root, &path, true)?;
        if doc.has_frontmatter {
            report.documents_with_frontmatter += 1;
        }
        let lint = lint_metadata_document(&config.knowledge_root, &doc);
        if lint.metadata_valid {
            report.valid_docs += 1;
        }
        for issue in &lint.issues {
            match issue.severity.as_str() {
                "error" => report.error_count += 1,
                "warning" => report.warning_count += 1,
                _ => {}
            }
        }
        report.documents.push(lint);
    }

    Ok(report)
}

/// 为指定文档生成推断后的 frontmatter 模板，但不写回文件。
pub fn metadata_template_for_document(
    root: &Path,
    path: &Path,
) -> Result<MetadataTemplateResponse> {
    let doc = load_document(root, path, true)?;
    let metadata = infer_metadata_template(&doc);
    let frontmatter = render_frontmatter(&metadata)?;
    Ok(MetadataTemplateResponse {
        path: doc.relative_path,
        has_frontmatter: doc.has_frontmatter,
        metadata,
        frontmatter,
        frontmatter_span: doc.frontmatter_span,
        insert_before_line: 1,
    })
}

/// 检查单个文档的 frontmatter 语法与规范问题。
pub fn check_metadata_document(root: &Path, path: &Path) -> Result<MetadataCheckResponse> {
    let doc = load_document(root, path, true)?;
    let lint = lint_metadata_document(root, &doc);
    let (error_count, warning_count) = count_issue_severities(&lint.issues);
    Ok(MetadataCheckResponse {
        path: doc.relative_path,
        has_frontmatter: doc.has_frontmatter,
        metadata_valid: lint.metadata_valid,
        parse_error: doc.metadata_parse_error,
        metadata: doc.metadata,
        issues: lint.issues,
        frontmatter_span: doc.frontmatter_span,
        insert_before_line: 1,
        error_count,
        warning_count,
    })
}

/// 读取单个 Markdown 文档，并可选解析 frontmatter。
pub fn load_document(
    root: &Path,
    path: &Path,
    parse_frontmatter_enabled: bool,
) -> Result<MarkdownDocument> {
    let raw_content = fs::read_to_string(path)
        .with_context(|| format!("failed to read markdown {}", path.display()))?;
    let relative_path = path
        .strip_prefix(root)
        .with_context(|| format!("path {} not under {}", path.display(), root.display()))?
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");

    let (
        content,
        metadata,
        has_frontmatter,
        metadata_parse_error,
        content_start_line,
        frontmatter_span,
    ) = if parse_frontmatter_enabled {
        parse_document_content(&raw_content)
    } else {
        (raw_content.clone(), None, false, None, 1, None)
    };

    Ok(MarkdownDocument {
        relative_path,
        absolute_path: path.to_path_buf(),
        raw_content,
        content,
        metadata,
        has_frontmatter,
        metadata_parse_error,
        content_start_line,
        frontmatter_span,
    })
}

/// 按标题构建 section 树；root 正文保留为空标题 section。
pub fn build_sections(doc: &MarkdownDocument) -> Vec<SectionDraft> {
    let mut sections = Vec::new();
    let mut headings: Vec<String> = Vec::new();
    let mut current_body: Vec<(usize, &str)> = Vec::new();
    let mut current_heading = String::new();
    let mut current_level = 0usize;
    let mut current_parent = String::new();
    let mut current_heading_line = doc.content_start_line;

    for (index, line) in doc.content.lines().enumerate() {
        let line_number = doc.content_start_line + index;
        if let Some((level, heading)) = parse_heading(line) {
            flush_section(
                &mut sections,
                &current_heading,
                current_level,
                &current_parent,
                current_heading_line,
                &current_body,
            );
            current_body.clear();

            headings.truncate(level.saturating_sub(1));
            current_parent = headings.join(" > ");
            headings.push(heading.to_string());
            current_heading = headings.join(" > ");
            current_level = level;
            current_heading_line = line_number;
        } else {
            current_body.push((line_number, line));
        }
    }

    flush_section(
        &mut sections,
        &current_heading,
        current_level,
        &current_parent,
        current_heading_line,
        &current_body,
    );
    sections
}

/// 按 section 与段落把 Markdown 切成适合检索的块。
pub fn chunk_document(doc: &MarkdownDocument, chunk_char_limit: usize) -> Vec<ChunkDraft> {
    let sections = build_sections(doc);
    let mut chunks = Vec::new();

    for (section_ordinal, section) in sections.iter().enumerate() {
        chunks.extend(chunk_section(
            doc,
            section_ordinal as i64,
            section,
            chunk_char_limit,
        ));
    }

    if chunks.is_empty() && !doc.content.trim().is_empty() {
        chunks.push(build_chunk(
            doc,
            0,
            0,
            "",
            doc.content.trim(),
            doc.content_start_line,
            doc.content_start_line + doc.content.lines().count().saturating_sub(1),
        ));
    }

    chunks
}

fn parse_document_content(
    raw_content: &str,
) -> (
    String,
    Option<DocumentMetadata>,
    bool,
    Option<String>,
    usize,
    Option<LineSpan>,
) {
    let Some(frontmatter) = extract_frontmatter_block(raw_content) else {
        if starts_with_frontmatter_delimiter(raw_content) {
            return (
                raw_content.to_string(),
                None,
                true,
                Some(
                    "frontmatter opening delimiter found but closing delimiter is missing"
                        .to_string(),
                ),
                1,
                None,
            );
        }
        return (raw_content.to_string(), None, false, None, 1, None);
    };

    let body = raw_content[frontmatter.body_start..].to_string();
    let frontmatter_span = Some(LineSpan {
        start_line: frontmatter.start_line,
        end_line: frontmatter.end_line,
    });
    match serde_yaml::from_str::<DocumentMetadata>(frontmatter.yaml) {
        Ok(mut metadata) => {
            normalize_metadata(&mut metadata);
            (
                body,
                Some(metadata),
                true,
                None,
                frontmatter.body_start_line,
                frontmatter_span,
            )
        }
        Err(err) => (
            body,
            None,
            true,
            Some(err.to_string()),
            frontmatter.body_start_line,
            frontmatter_span,
        ),
    }
}

fn infer_metadata_template(doc: &MarkdownDocument) -> DocumentMetadata {
    let title = infer_document_title(doc);
    let file_stem = doc
        .absolute_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let domain = infer_document_domain(&doc.relative_path);
    let tag_domain = domain.clone().unwrap_or_else(|| "planning".to_string());
    let tag_leaf = slugify_tag_leaf(&title)
        .or_else(|| slugify_tag_leaf(&file_stem))
        .unwrap_or_else(|| "general".to_string());

    let mut metadata = DocumentMetadata {
        title,
        tags: vec![format!("{tag_domain}/{tag_leaf}")],
        aliases: Vec::new(),
        related: Vec::new(),
        source_type: "note".to_string(),
        source_ref: format!("local://{}", doc.relative_path),
        status: "draft".to_string(),
        domain,
        keywords: Vec::new(),
        updated_by: Some("agent".to_string()),
        updated_at: None,
        review_priority: Some("medium".to_string()),
    };

    if !file_stem.is_empty() && !metadata.title.eq_ignore_ascii_case(&file_stem) {
        metadata.aliases.push(file_stem);
    }
    normalize_metadata(&mut metadata);
    metadata
}

fn infer_document_title(doc: &MarkdownDocument) -> String {
    doc.content
        .lines()
        .find_map(|line| parse_heading(line).map(|(_, heading)| heading.trim().to_string()))
        .filter(|heading| !heading.is_empty())
        .or_else(|| {
            doc.absolute_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.trim().to_string())
                .filter(|stem| !stem.is_empty())
        })
        .unwrap_or_else(|| "Untitled".to_string())
}

fn infer_document_domain(relative_path: &str) -> Option<String> {
    let first_segment = relative_path.split('/').next().unwrap_or_default();
    match first_segment {
        "CPP" => Some("cpp".to_string()),
        "Estimation" => Some("estimation".to_string()),
        "Math" => Some("math".to_string()),
        "System" => Some("system".to_string()),
        "GCOPTER" => Some("planning".to_string()),
        _ if relative_path == "PLAN.md" => Some("planning".to_string()),
        _ => None,
    }
}

fn slugify_tag_leaf(value: &str) -> Option<String> {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if (ch.is_ascii_whitespace() || matches!(ch, '-' | '_' | '/'))
            && !last_dash
            && !slug.is_empty()
        {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    (!slug.is_empty()).then_some(slug)
}

fn render_frontmatter(metadata: &DocumentMetadata) -> Result<String> {
    let yaml = serde_yaml::to_string(metadata)
        .context("failed to serialize metadata to YAML")?
        .trim()
        .trim_start_matches("---\n")
        .trim_start_matches("---\r\n")
        .trim()
        .to_string();
    Ok(format!("---\n{yaml}\n---"))
}

fn lint_metadata_document(root: &Path, doc: &MarkdownDocument) -> MetadataLintDocument {
    let mut issues = Vec::new();

    if let Some(error) = &doc.metadata_parse_error {
        issues.push(error_issue(
            "invalid_frontmatter",
            format!("frontmatter YAML parse failed: {error}"),
        ));
    }

    if !doc.has_frontmatter {
        issues.push(error_issue(
            "missing_frontmatter",
            "frontmatter block is missing at document head".to_string(),
        ));
    }

    if let Some(metadata) = &doc.metadata {
        if metadata.title.is_empty() {
            issues.push(error_issue(
                "missing_title",
                "title is required".to_string(),
            ));
        }
        if metadata.tags.is_empty() {
            issues.push(error_issue("missing_tags", "tags is required".to_string()));
        }
        if metadata.source_type.is_empty() {
            issues.push(error_issue(
                "missing_source_type",
                "source_type is required".to_string(),
            ));
        } else if !SOURCE_TYPES.contains(&metadata.source_type.as_str()) {
            issues.push(error_issue(
                "invalid_source_type",
                format!("unsupported source_type: {}", metadata.source_type),
            ));
        }
        if metadata.source_ref.is_empty() {
            issues.push(error_issue(
                "missing_source_ref",
                "source_ref is required".to_string(),
            ));
        }
        if metadata.status.is_empty() {
            issues.push(error_issue(
                "missing_status",
                "status is required".to_string(),
            ));
        } else if !STATUS_VALUES.contains(&metadata.status.as_str()) {
            issues.push(error_issue(
                "invalid_status",
                format!("unsupported status: {}", metadata.status),
            ));
        }

        if let Some(review_priority) = &metadata.review_priority {
            if !REVIEW_PRIORITIES.contains(&review_priority.as_str()) {
                issues.push(error_issue(
                    "invalid_review_priority",
                    format!("unsupported review_priority: {review_priority}"),
                ));
            }
        }

        for tag in &metadata.tags {
            if !is_valid_tag(tag) {
                issues.push(error_issue(
                    "invalid_tag",
                    format!("invalid tag syntax: {tag}"),
                ));
            }
        }

        for related in &metadata.related {
            let candidate = root.join(related);
            if !candidate.is_file() {
                issues.push(error_issue(
                    "missing_related_target",
                    format!("related target does not exist: {related}"),
                ));
            }
        }

        let file_stem = doc
            .absolute_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .trim();
        let has_alias_match = metadata
            .aliases
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(file_stem));
        if !file_stem.is_empty()
            && !metadata.title.is_empty()
            && !metadata.title.eq_ignore_ascii_case(file_stem)
            && !has_alias_match
        {
            issues.push(warning_issue(
                "title_alias_gap",
                format!("title differs from file stem `{file_stem}` but aliases does not cover it"),
            ));
        }

        if let Some(domain) = &metadata.domain {
            if !KNOWN_TAG_DOMAINS.contains(&domain.as_str()) {
                issues.push(warning_issue(
                    "unknown_domain",
                    format!("domain is outside the recommended taxonomy: {domain}"),
                ));
            }
            if let Some(tag_domain) = metadata.tags.iter().find_map(|tag| tag.split('/').next()) {
                if tag_domain != domain {
                    issues.push(warning_issue(
                        "domain_tag_mismatch",
                        format!("domain `{domain}` does not match first tag domain `{tag_domain}`"),
                    ));
                }
            }
        }

        if !metadata.keywords.is_empty()
            && !metadata.tags.is_empty()
            && !keywords_overlap_tags(&metadata.keywords, &metadata.tags)
        {
            issues.push(warning_issue(
                "keywords_tag_gap",
                "keywords do not overlap with any tag leaf tokens".to_string(),
            ));
        }

        if metadata.related.len() > 12 {
            issues.push(warning_issue(
                "related_too_many",
                format!(
                    "related contains {} items; consider keeping only strong links",
                    metadata.related.len()
                ),
            ));
        }

        if metadata.status == "seed" && doc.content.chars().count() > 2_000 {
            issues.push(warning_issue(
                "seed_doc_too_long",
                "status=seed but body is already long; consider promoting or splitting".to_string(),
            ));
        }
    }

    let metadata_valid = issues.iter().all(|issue| issue.severity != "error");
    MetadataLintDocument {
        path: doc.relative_path.clone(),
        has_frontmatter: doc.has_frontmatter,
        metadata_valid,
        issues,
    }
}

fn chunk_section(
    doc: &MarkdownDocument,
    section_ordinal: i64,
    section: &SectionDraft,
    chunk_char_limit: usize,
) -> Vec<ChunkDraft> {
    let paragraphs = split_paragraph_slices(&section.body_text, section.body_start_line);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_start_line = 0usize;
    let mut current_end_line = 0usize;
    let mut chunk_ordinal_in_section = 0i64;

    for paragraph in paragraphs {
        let paragraph_text = paragraph.text.trim();
        if paragraph_text.is_empty() {
            continue;
        }

        let candidate_len = current.len() + paragraph_text.len() + 2;
        if !current.is_empty() && candidate_len > chunk_char_limit {
            chunks.push(build_chunk(
                doc,
                section_ordinal,
                chunk_ordinal_in_section,
                &section.heading_path,
                &current,
                current_start_line,
                current_end_line,
            ));
            chunk_ordinal_in_section += 1;
            current.clear();
        }

        if paragraph_text.len() > chunk_char_limit {
            if !current.is_empty() {
                chunks.push(build_chunk(
                    doc,
                    section_ordinal,
                    chunk_ordinal_in_section,
                    &section.heading_path,
                    &current,
                    current_start_line,
                    current_end_line,
                ));
                chunk_ordinal_in_section += 1;
                current.clear();
            }
            for piece in split_long_text(paragraph_text, chunk_char_limit) {
                chunks.push(build_chunk(
                    doc,
                    section_ordinal,
                    chunk_ordinal_in_section,
                    &section.heading_path,
                    &piece,
                    paragraph.start_line,
                    paragraph.end_line,
                ));
                chunk_ordinal_in_section += 1;
            }
            continue;
        }

        if current.is_empty() {
            current_start_line = paragraph.start_line;
        } else {
            current.push_str("\n\n");
        }
        current_end_line = paragraph.end_line;
        current.push_str(paragraph_text);
    }

    if !current.trim().is_empty() {
        chunks.push(build_chunk(
            doc,
            section_ordinal,
            chunk_ordinal_in_section,
            &section.heading_path,
            &current,
            current_start_line,
            current_end_line,
        ));
    }

    chunks
}

fn build_chunk(
    doc: &MarkdownDocument,
    section_ordinal: i64,
    chunk_ordinal_in_section: i64,
    heading_path: &str,
    body: &str,
    start_line: usize,
    end_line: usize,
) -> ChunkDraft {
    let mut text = String::new();
    text.push_str("Path: ");
    text.push_str(&doc.relative_path);
    text.push('\n');
    if !heading_path.is_empty() {
        text.push_str("Heading: ");
        text.push_str(heading_path);
        text.push_str("\n\n");
    } else {
        text.push('\n');
    }
    text.push_str(body.trim());

    ChunkDraft {
        section_ordinal,
        chunk_ordinal_in_section,
        heading_path: heading_path.to_string(),
        text,
        start_line,
        end_line,
    }
}

fn flush_section(
    sections: &mut Vec<SectionDraft>,
    heading_path: &str,
    heading_level: usize,
    parent_heading_path: &str,
    heading_line: usize,
    lines: &[(usize, &str)],
) {
    let body_text = lines
        .iter()
        .map(|(_, line)| *line)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if body_text.is_empty() {
        return;
    }

    let body_start_line = lines
        .iter()
        .find_map(|(line_number, line)| (!line.trim().is_empty()).then_some(*line_number))
        .unwrap_or(heading_line);
    let end_line = lines
        .iter()
        .rev()
        .find_map(|(line_number, line)| (!line.trim().is_empty()).then_some(*line_number))
        .unwrap_or(body_start_line);
    let first_paragraph = split_paragraphs(&body_text)
        .into_iter()
        .next()
        .unwrap_or_default();
    sections.push(SectionDraft {
        heading_path: heading_path.to_string(),
        heading_level: heading_level as i64,
        parent_heading_path: parent_heading_path.to_string(),
        body_text,
        first_paragraph,
        heading_line: if heading_path.is_empty() {
            body_start_line
        } else {
            heading_line
        },
        body_start_line,
        end_line,
    });
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|&ch| ch == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed.get(hashes..)?.trim();
    if rest.is_empty() {
        return None;
    }
    Some((hashes, rest))
}

fn split_paragraphs(body: &str) -> Vec<String> {
    split_paragraph_slices(body, 1)
        .into_iter()
        .map(|paragraph| paragraph.text)
        .collect()
}

fn split_long_text(text: &str, limit: usize) -> Vec<String> {
    if text.chars().count() <= limit {
        return vec![text.to_string()];
    }

    let chars = text.chars().collect::<Vec<_>>();
    chars
        .chunks(limit)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect()
}

fn split_paragraph_slices(body: &str, base_line: usize) -> Vec<ParagraphSlice> {
    let mut paragraphs = Vec::new();
    let mut current = Vec::new();
    let mut current_start_line = 0usize;
    let mut current_end_line = 0usize;

    for (index, line) in body.lines().enumerate() {
        let line_number = base_line + index;
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(ParagraphSlice {
                    text: current.join("\n"),
                    start_line: current_start_line,
                    end_line: current_end_line,
                });
                current.clear();
            }
            continue;
        }

        if current.is_empty() {
            current_start_line = line_number;
        }
        current_end_line = line_number;
        current.push(line.to_string());
    }

    if !current.is_empty() {
        paragraphs.push(ParagraphSlice {
            text: current.join("\n"),
            start_line: current_start_line,
            end_line: current_end_line,
        });
    }

    paragraphs
}

fn count_issue_severities(issues: &[MetadataLintIssue]) -> (usize, usize) {
    let mut error_count = 0usize;
    let mut warning_count = 0usize;
    for issue in issues {
        match issue.severity.as_str() {
            "error" => error_count += 1,
            "warning" => warning_count += 1,
            _ => {}
        }
    }
    (error_count, warning_count)
}

fn should_descend(entry: &DirEntry, exclude_hidden: bool, exclude_obsidian_dir: bool) -> bool {
    let name = entry.file_name().to_string_lossy();
    if exclude_obsidian_dir && entry.file_type().is_dir() && name == ".obsidian" {
        return false;
    }
    if exclude_hidden && entry.depth() > 0 && name.starts_with('.') {
        return false;
    }
    true
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn extract_frontmatter_block(raw_content: &str) -> Option<FrontmatterBlock<'_>> {
    let mut lines = raw_content.split_inclusive('\n');
    let first = lines.next()?;
    if trim_line_breaks(first) != "---" {
        return None;
    }

    let yaml_start = first.len();
    let mut cursor = yaml_start;
    let mut line_number = 2usize;
    for line in raw_content[yaml_start..].split_inclusive('\n') {
        let line_start = cursor;
        let line_end = cursor + line.len();
        if trim_line_breaks(line) == "---" {
            return Some(FrontmatterBlock {
                yaml: &raw_content[yaml_start..line_start],
                body_start: line_end,
                start_line: 1,
                end_line: line_number,
                body_start_line: line_number + 1,
            });
        }
        cursor = line_end;
        line_number += 1;
    }

    None
}

fn starts_with_frontmatter_delimiter(raw_content: &str) -> bool {
    raw_content
        .lines()
        .next()
        .is_some_and(|line| line.trim_end_matches('\r') == "---")
}

fn normalize_metadata(metadata: &mut DocumentMetadata) {
    metadata.title = metadata.title.trim().to_string();
    metadata.tags = normalize_string_list(
        metadata
            .tags
            .iter()
            .map(|tag| tag.trim().to_ascii_lowercase())
            .collect(),
    );
    metadata.aliases = normalize_string_list(
        metadata
            .aliases
            .iter()
            .map(|alias| alias.trim().to_string())
            .collect(),
    );
    metadata.related = normalize_string_list(
        metadata
            .related
            .iter()
            .map(|path| normalize_related_path(path))
            .collect(),
    );
    metadata.source_type = metadata.source_type.trim().to_ascii_lowercase();
    metadata.source_ref = metadata.source_ref.trim().to_string();
    metadata.status = metadata.status.trim().to_ascii_lowercase();
    metadata.domain = normalize_optional_lower(&metadata.domain);
    metadata.keywords = normalize_string_list(
        metadata
            .keywords
            .iter()
            .map(|keyword| keyword.trim().to_string())
            .collect(),
    );
    metadata.updated_by = normalize_optional_lower(&metadata.updated_by);
    metadata.updated_at = normalize_optional_trim(&metadata.updated_at);
    metadata.review_priority = normalize_optional_lower(&metadata.review_priority);
}

fn normalize_optional_lower(value: &Option<String>) -> Option<String> {
    value.as_ref().and_then(|item| {
        let trimmed = item.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_ascii_lowercase())
    })
}

fn normalize_optional_trim(value: &Option<String>) -> Option<String> {
    value.as_ref().and_then(|item| {
        let trimmed = item.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    let values = values
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>();
    values.into_iter().collect()
}

fn normalize_related_path(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

fn keywords_overlap_tags(keywords: &[String], tags: &[String]) -> bool {
    let keyword_set = keywords
        .iter()
        .map(|keyword| keyword.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    tags.iter().any(|tag| {
        tag.rsplit('/')
            .next()
            .map(|leaf| keyword_set.contains(leaf))
            .unwrap_or(false)
    })
}

fn is_valid_tag(tag: &str) -> bool {
    if tag.is_empty() || tag.starts_with('/') || tag.ends_with('/') || tag.contains("//") {
        return false;
    }

    tag.split('/').all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    })
}

fn trim_line_breaks(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn error_issue(code: &str, message: String) -> MetadataLintIssue {
    MetadataLintIssue {
        severity: "error".to_string(),
        code: code.to_string(),
        message,
    }
}

fn warning_issue(code: &str, message: String) -> MetadataLintIssue {
    MetadataLintIssue {
        severity: "warning".to_string(),
        code: code.to_string(),
        message,
    }
}

struct FrontmatterBlock<'a> {
    yaml: &'a str,
    body_start: usize,
    start_line: usize,
    end_line: usize,
    body_start_line: usize,
}

struct ParagraphSlice {
    text: String,
    start_line: usize,
    end_line: usize,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::config::AppConfig;

    fn test_config(root: &Path) -> AppConfig {
        AppConfig {
            knowledge_root: root.to_path_buf(),
            state_dir: root.join("state"),
            database_path: root.join("state/index.sqlite3"),
            embedding_backend: "hashing".to_string(),
            fastembed_model: "MultilingualE5Small".to_string(),
            embedding_cache_dir: root.join("state/fastembed"),
            hashing_dimensions: 128,
            chunk_char_limit: 120,
            search_limit: 8,
            exclude_hidden: true,
            exclude_obsidian_dir: true,
            metadata_frontmatter_enabled: true,
            graph_enabled: true,
            graph_semantic_neighbors_per_node: 6,
            graph_semantic_min_score: 0.42,
        }
    }

    #[test]
    fn chunks_follow_heading_boundaries() {
        let doc = MarkdownDocument {
            relative_path: "demo.md".to_string(),
            absolute_path: PathBuf::from("demo.md"),
            raw_content: "# A\nline1\n\nline2\n## B\nline3".to_string(),
            content: "# A\nline1\n\nline2\n## B\nline3".to_string(),
            metadata: None,
            has_frontmatter: false,
            metadata_parse_error: None,
            content_start_line: 1,
            frontmatter_span: None,
        };

        let chunks = chunk_document(&doc, 32);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].text.contains("Heading: A"));
        assert!(chunks[1].text.contains("Heading: A > B"));
    }

    #[test]
    fn build_sections_tracks_parent_headings() {
        let doc = MarkdownDocument {
            relative_path: "demo.md".to_string(),
            absolute_path: PathBuf::from("demo.md"),
            raw_content: "intro\n# A\nline1\n## B\nline2\n### C\nline3".to_string(),
            content: "intro\n# A\nline1\n## B\nline2\n### C\nline3".to_string(),
            metadata: None,
            has_frontmatter: false,
            metadata_parse_error: None,
            content_start_line: 1,
            frontmatter_span: None,
        };

        let sections = build_sections(&doc);
        assert_eq!(sections.len(), 4);
        assert_eq!(sections[0].heading_path, "");
        assert_eq!(sections[0].heading_level, 0);
        assert_eq!(sections[1].heading_path, "A");
        assert_eq!(sections[1].parent_heading_path, "");
        assert_eq!(sections[2].heading_path, "A > B");
        assert_eq!(sections[2].parent_heading_path, "A");
        assert_eq!(sections[3].heading_path, "A > B > C");
        assert_eq!(sections[3].parent_heading_path, "A > B");
    }

    #[test]
    fn load_document_tracks_frontmatter_span_and_body_start() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        fs::write(
            &path,
            "---\ntitle: Demo\ntags:\n  - cpp/memory\nsource_type: note\nsource_ref: local://demo\nstatus: draft\n---\n# Body\ncontent",
        )
        .unwrap();

        let doc = load_document(temp.path(), &path, true).unwrap();
        assert_eq!(
            doc.frontmatter_span,
            Some(LineSpan {
                start_line: 1,
                end_line: 8
            })
        );
        assert_eq!(doc.content_start_line, 9);
    }

    #[test]
    fn build_sections_tracks_original_line_numbers() {
        let doc = MarkdownDocument {
            relative_path: "demo.md".to_string(),
            absolute_path: PathBuf::from("demo.md"),
            raw_content: "intro\n# A\nline1\n## B\nline2\n### C\nline3".to_string(),
            content: "intro\n# A\nline1\n## B\nline2\n### C\nline3".to_string(),
            metadata: None,
            has_frontmatter: false,
            metadata_parse_error: None,
            content_start_line: 1,
            frontmatter_span: None,
        };

        let sections = build_sections(&doc);
        assert_eq!(sections[0].heading_line, 1);
        assert_eq!(sections[0].body_start_line, 1);
        assert_eq!(sections[0].end_line, 1);
        assert_eq!(sections[1].heading_line, 2);
        assert_eq!(sections[1].body_start_line, 3);
        assert_eq!(sections[1].end_line, 3);
        assert_eq!(sections[2].heading_line, 4);
        assert_eq!(sections[2].body_start_line, 5);
        assert_eq!(sections[2].end_line, 5);
        assert_eq!(sections[3].heading_line, 6);
        assert_eq!(sections[3].body_start_line, 7);
        assert_eq!(sections[3].end_line, 7);
    }

    #[test]
    fn chunk_document_preserves_chunk_line_spans() {
        let doc = MarkdownDocument {
            relative_path: "demo.md".to_string(),
            absolute_path: PathBuf::from("demo.md"),
            raw_content: "# A\nalpha\n\nbeta gamma\n## B\nomega".to_string(),
            content: "# A\nalpha\n\nbeta gamma\n## B\nomega".to_string(),
            metadata: None,
            has_frontmatter: false,
            metadata_parse_error: None,
            content_start_line: 1,
            frontmatter_span: None,
        };

        let chunks = chunk_document(&doc, 10);
        assert_eq!(chunks.len(), 3);
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (2, 2));
        assert_eq!((chunks[1].start_line, chunks[1].end_line), (4, 4));
        assert_eq!((chunks[2].start_line, chunks[2].end_line), (6, 6));
    }

    #[test]
    fn check_metadata_reports_insert_position_when_frontmatter_missing() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        fs::write(&path, "# Demo\nbody").unwrap();

        let response = check_metadata_document(temp.path(), &path).unwrap();
        assert_eq!(response.frontmatter_span, None);
        assert_eq!(response.insert_before_line, 1);
        assert_eq!(response.error_count, 1);
        assert_eq!(response.warning_count, 0);
    }

    #[test]
    fn load_document_strips_valid_frontmatter() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("note.md");
        fs::write(
            &path,
            "---\ntitle: Demo\ntags:\n  - cpp/memory\nsource_type: note\nsource_ref: local://demo\nstatus: draft\n---\n# Body\ncontent",
        )
        .unwrap();

        let doc = load_document(temp.path(), &path, true).unwrap();
        assert!(doc.has_frontmatter);
        assert_eq!(doc.content, "# Body\ncontent");
        assert_eq!(doc.metadata.unwrap().title, "Demo");
    }

    #[test]
    fn lint_reports_missing_frontmatter() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("note.md"), "# Body\ncontent").unwrap();

        let config = test_config(temp.path());

        let report = lint_metadata_tree(&config).unwrap();
        assert_eq!(report.error_count, 1);
        assert_eq!(report.documents[0].issues[0].code, "missing_frontmatter");
    }

    #[test]
    fn lint_reports_invalid_related_target() {
        let temp = TempDir::new().unwrap();
        fs::write(
            temp.path().join("note.md"),
            "---\ntitle: Demo\ntags:\n  - cpp/memory\nrelated:\n  - missing.md\nsource_type: note\nsource_ref: local://demo\nstatus: stable\n---\n# Body\ncontent",
        )
        .unwrap();

        let config = test_config(temp.path());

        let report = lint_metadata_tree(&config).unwrap();
        assert_eq!(report.error_count, 1);
        assert!(
            report.documents[0]
                .issues
                .iter()
                .any(|issue| issue.code == "missing_related_target")
        );
    }

    #[test]
    fn metadata_template_infers_title_domain_and_tag() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("System")).unwrap();
        let path = temp.path().join("System/ROS.md");
        fs::write(&path, "# ROS\nbody").unwrap();

        let response = metadata_template_for_document(temp.path(), &path).unwrap();
        assert_eq!(response.path, "System/ROS.md");
        assert_eq!(response.metadata.title, "ROS");
        assert_eq!(response.metadata.domain.as_deref(), Some("system"));
        assert_eq!(response.metadata.tags, vec!["system/ros".to_string()]);
        assert!(response.frontmatter.starts_with("---\n"));
    }

    #[test]
    fn discovery_skips_hidden_and_obsidian() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("visible")).unwrap();
        fs::create_dir_all(temp.path().join(".obsidian")).unwrap();
        fs::create_dir_all(temp.path().join(".hidden")).unwrap();
        fs::write(temp.path().join("visible/note.md"), "hello").unwrap();
        fs::write(temp.path().join(".obsidian/app.md"), "ignored").unwrap();
        fs::write(temp.path().join(".hidden/secret.md"), "ignored").unwrap();

        let config = test_config(temp.path());

        let files = discover_markdown_files(&config).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("visible/note.md"));
    }
}
