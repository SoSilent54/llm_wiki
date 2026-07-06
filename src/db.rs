use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{
    ChunkRecord, DocumentEmbeddingRecord, DocumentManifest, GraphEdgeRecord, GraphNodeRecord,
    OverviewBucket, SectionEmbeddingRecord, SectionRecord,
};

#[derive(Debug, Clone)]
pub struct IndexDatabase {
    db_path: PathBuf,
}

impl IndexDatabase {
    pub fn new(db_path: impl AsRef<Path>) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
        }
    }

    /// 初始化 SQLite schema。
    pub fn init(&self) -> Result<()> {
        let conn = self.open()?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS documents (
                doc_path TEXT PRIMARY KEY,
                file_hash TEXT NOT NULL,
                embedding_fingerprint TEXT NOT NULL DEFAULT '',
                indexed_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sections (
                section_id TEXT PRIMARY KEY,
                doc_path TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                heading_path TEXT NOT NULL,
                heading_level INTEGER NOT NULL,
                parent_heading_path TEXT NOT NULL,
                body_text TEXT NOT NULL,
                first_paragraph TEXT NOT NULL,
                section_hash TEXT NOT NULL,
                heading_line INTEGER NOT NULL DEFAULT 0,
                body_start_line INTEGER NOT NULL DEFAULT 0,
                end_line INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY(doc_path) REFERENCES documents(doc_path) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_sections_doc_path ON sections(doc_path, ordinal);
            CREATE TABLE IF NOT EXISTS doc_embeddings (
                doc_path TEXT PRIMARY KEY,
                embedding_json TEXT NOT NULL,
                embedding_dim INTEGER NOT NULL,
                embedding_fingerprint TEXT NOT NULL,
                FOREIGN KEY(doc_path) REFERENCES documents(doc_path) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS section_embeddings (
                section_id TEXT PRIMARY KEY,
                doc_path TEXT NOT NULL,
                heading_path TEXT NOT NULL,
                first_paragraph TEXT NOT NULL,
                embedding_json TEXT NOT NULL,
                embedding_dim INTEGER NOT NULL,
                embedding_fingerprint TEXT NOT NULL,
                FOREIGN KEY(doc_path) REFERENCES documents(doc_path) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_section_embeddings_doc_path ON section_embeddings(doc_path);
            CREATE TABLE IF NOT EXISTS graph_nodes (
                node_id TEXT PRIMARY KEY,
                node_type TEXT NOT NULL,
                ref_path TEXT NOT NULL,
                ref_section TEXT NOT NULL,
                label TEXT NOT NULL,
                payload_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_graph_nodes_type ON graph_nodes(node_type);
            CREATE INDEX IF NOT EXISTS idx_graph_nodes_ref_path ON graph_nodes(ref_path);
            CREATE TABLE IF NOT EXISTS graph_edges (
                edge_id TEXT PRIMARY KEY,
                src_node_id TEXT NOT NULL,
                dst_node_id TEXT NOT NULL,
                edge_type TEXT NOT NULL,
                weight REAL NOT NULL,
                evidence_json TEXT NOT NULL,
                graph_fingerprint TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_graph_edges_src_type ON graph_edges(src_node_id, edge_type);
            CREATE INDEX IF NOT EXISTS idx_graph_edges_dst_type ON graph_edges(dst_node_id, edge_type);
            CREATE INDEX IF NOT EXISTS idx_graph_edges_type_weight ON graph_edges(edge_type, weight);
            CREATE TABLE IF NOT EXISTS chunks (
                chunk_id TEXT PRIMARY KEY,
                section_id TEXT NOT NULL DEFAULT '',
                doc_path TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                chunk_ordinal_in_section INTEGER NOT NULL DEFAULT 0,
                heading_path TEXT NOT NULL,
                chunk_hash TEXT NOT NULL,
                text TEXT NOT NULL,
                start_line INTEGER NOT NULL DEFAULT 0,
                end_line INTEGER NOT NULL DEFAULT 0,
                embedding_json TEXT NOT NULL,
                FOREIGN KEY(doc_path) REFERENCES documents(doc_path) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_doc_path ON chunks(doc_path);
            "#,
        )?;
        self.ensure_document_columns(&conn)?;
        self.ensure_section_columns(&conn)?;
        self.ensure_chunk_columns(&conn)?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_chunks_section_id ON chunks(section_id)",
            [],
        )?;
        Ok(())
    }

    pub fn document_manifests(&self) -> Result<HashMap<String, DocumentManifest>> {
        let conn = self.open()?;
        let mut stmt =
            conn.prepare("SELECT doc_path, file_hash, embedding_fingerprint FROM documents")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                DocumentManifest {
                    file_hash: row.get(1)?,
                    embedding_fingerprint: row.get(2)?,
                },
            ))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (path, manifest) = row?;
            map.insert(path, manifest);
        }
        Ok(map)
    }

    pub fn document_paths(&self) -> Result<HashSet<String>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare("SELECT doc_path FROM documents")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut set = HashSet::new();
        for row in rows {
            set.insert(row?);
        }
        Ok(set)
    }

    pub fn has_sections_for_doc(&self, doc_path: &str) -> Result<bool> {
        let conn = self.open()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM sections WHERE doc_path = ?1",
            params![doc_path],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    pub fn has_doc_embedding_for_doc(&self, doc_path: &str) -> Result<bool> {
        let conn = self.open()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM doc_embeddings WHERE doc_path = ?1",
            params![doc_path],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    pub fn has_section_embeddings_for_doc(&self, doc_path: &str) -> Result<bool> {
        let conn = self.open()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM section_embeddings WHERE doc_path = ?1",
            params![doc_path],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    pub fn has_section_anchors_for_doc(&self, doc_path: &str) -> Result<bool> {
        let conn = self.open()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM sections WHERE doc_path = ?1",
            params![doc_path],
            |row| row.get::<_, i64>(0),
        )?;
        if count == 0 {
            return Ok(false);
        }
        let anchored = conn.query_row(
            "SELECT COUNT(*) FROM sections WHERE doc_path = ?1 AND heading_line > 0 AND body_start_line > 0 AND end_line >= body_start_line",
            params![doc_path],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(anchored == count)
    }

    pub fn has_chunk_anchors_for_doc(&self, doc_path: &str) -> Result<bool> {
        let conn = self.open()?;
        let count = conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE doc_path = ?1",
            params![doc_path],
            |row| row.get::<_, i64>(0),
        )?;
        if count == 0 {
            return Ok(false);
        }
        let anchored = conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE doc_path = ?1 AND start_line > 0 AND end_line >= start_line",
            params![doc_path],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(anchored == count)
    }

    /// 用事务替换单个文档、其 section、各层向量及全部块。
    pub fn replace_document(
        &self,
        doc_path: &str,
        file_hash: &str,
        embedding_fingerprint: &str,
        doc_embedding: &DocumentEmbeddingRecord,
        sections: &[SectionRecord],
        section_embeddings: &[SectionEmbeddingRecord],
        chunks: &[ChunkRecord],
    ) -> Result<()> {
        let mut conn = self.open()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO documents(doc_path, file_hash, embedding_fingerprint, indexed_at) VALUES (?1, ?2, ?3, unixepoch()) \
             ON CONFLICT(doc_path) DO UPDATE SET file_hash = excluded.file_hash, embedding_fingerprint = excluded.embedding_fingerprint, indexed_at = excluded.indexed_at",
            params![doc_path, file_hash, embedding_fingerprint],
        )?;
        tx.execute("DELETE FROM chunks WHERE doc_path = ?1", params![doc_path])?;
        tx.execute(
            "DELETE FROM section_embeddings WHERE doc_path = ?1",
            params![doc_path],
        )?;
        tx.execute(
            "DELETE FROM doc_embeddings WHERE doc_path = ?1",
            params![doc_path],
        )?;
        tx.execute(
            "DELETE FROM sections WHERE doc_path = ?1",
            params![doc_path],
        )?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO sections(section_id, doc_path, ordinal, heading_path, heading_level, parent_heading_path, body_text, first_paragraph, section_hash, heading_line, body_start_line, end_line) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?;
            for section in sections {
                stmt.execute(params![
                    section.section_id,
                    section.doc_path,
                    section.ordinal,
                    section.heading_path,
                    section.heading_level,
                    section.parent_heading_path,
                    section.body_text,
                    section.first_paragraph,
                    section.section_hash,
                    section.heading_line as i64,
                    section.body_start_line as i64,
                    section.end_line as i64,
                ])?;
            }
        }

        tx.execute(
            "INSERT INTO doc_embeddings(doc_path, embedding_json, embedding_dim, embedding_fingerprint) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                doc_embedding.doc_path,
                serde_json::to_string(&doc_embedding.embedding)?,
                doc_embedding.embedding.len() as i64,
                embedding_fingerprint,
            ],
        )?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO section_embeddings(section_id, doc_path, heading_path, first_paragraph, embedding_json, embedding_dim, embedding_fingerprint) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for section in section_embeddings {
                stmt.execute(params![
                    section.section_id,
                    section.doc_path,
                    section.heading_path,
                    section.first_paragraph,
                    serde_json::to_string(&section.embedding)?,
                    section.embedding.len() as i64,
                    embedding_fingerprint,
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO chunks(chunk_id, section_id, doc_path, ordinal, chunk_ordinal_in_section, heading_path, chunk_hash, text, start_line, end_line, embedding_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for chunk in chunks {
                stmt.execute(params![
                    chunk.chunk_id,
                    chunk.section_id,
                    chunk.doc_path,
                    chunk.ordinal,
                    chunk.chunk_ordinal_in_section,
                    chunk.heading_path,
                    chunk.chunk_hash,
                    chunk.text,
                    chunk.start_line as i64,
                    chunk.end_line as i64,
                    serde_json::to_string(&chunk.embedding)?,
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// 全量替换图谱层；当前库规模较小，直接重建最稳妥。
    pub fn replace_graph(
        &self,
        nodes: &[GraphNodeRecord],
        edges: &[GraphEdgeRecord],
    ) -> Result<()> {
        let mut conn = self.open()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM graph_edges", [])?;
        tx.execute("DELETE FROM graph_nodes", [])?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO graph_nodes(node_id, node_type, ref_path, ref_section, label, payload_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for node in nodes {
                stmt.execute(params![
                    node.node_id,
                    node.node_type,
                    node.ref_path,
                    node.ref_section,
                    node.label,
                    node.payload_json,
                ])?;
            }
        }

        {
            let mut stmt = tx.prepare(
                "INSERT INTO graph_edges(edge_id, src_node_id, dst_node_id, edge_type, weight, evidence_json, graph_fingerprint) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for edge in edges {
                stmt.execute(params![
                    edge.edge_id,
                    edge.src_node_id,
                    edge.dst_node_id,
                    edge.edge_type,
                    edge.weight,
                    edge.evidence_json,
                    edge.graph_fingerprint,
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn delete_documents(&self, doc_paths: &[String]) -> Result<usize> {
        if doc_paths.is_empty() {
            return Ok(0);
        }

        let mut conn = self.open()?;
        let tx = conn.transaction()?;
        let mut deleted = 0;
        for doc_path in doc_paths {
            deleted += tx.execute(
                "DELETE FROM documents WHERE doc_path = ?1",
                params![doc_path],
            )?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    pub fn load_all_chunks(&self) -> Result<Vec<ChunkRecord>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT chunk_id, section_id, doc_path, ordinal, chunk_ordinal_in_section, heading_path, chunk_hash, text, start_line, end_line, embedding_json \
             FROM chunks ORDER BY doc_path, ordinal",
        )?;
        let rows = stmt.query_map([], |row| {
            let embedding_json: String = row.get(10)?;
            let embedding: Vec<f32> = serde_json::from_str(&embedding_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;

            Ok(ChunkRecord {
                chunk_id: row.get(0)?,
                section_id: row.get(1)?,
                doc_path: row.get(2)?,
                ordinal: row.get(3)?,
                chunk_ordinal_in_section: row.get(4)?,
                heading_path: row.get(5)?,
                chunk_hash: row.get(6)?,
                text: row.get(7)?,
                start_line: row.get::<_, i64>(8)? as usize,
                end_line: row.get::<_, i64>(9)? as usize,
                embedding,
            })
        })?;

        let mut chunks = Vec::new();
        for row in rows {
            chunks.push(row?);
        }
        Ok(chunks)
    }

    pub fn load_all_section_embeddings(&self) -> Result<Vec<SectionEmbeddingRecord>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT section_id, doc_path, heading_path, first_paragraph, embedding_json \
             FROM section_embeddings ORDER BY doc_path, heading_path",
        )?;
        let rows = stmt.query_map([], |row| {
            let embedding_json: String = row.get(4)?;
            let embedding: Vec<f32> = serde_json::from_str(&embedding_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;

            Ok(SectionEmbeddingRecord {
                section_id: row.get(0)?,
                doc_path: row.get(1)?,
                heading_path: row.get(2)?,
                first_paragraph: row.get(3)?,
                embedding,
            })
        })?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    pub fn load_all_sections(&self) -> Result<Vec<SectionRecord>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT section_id, doc_path, ordinal, heading_path, heading_level, parent_heading_path, body_text, first_paragraph, section_hash, heading_line, body_start_line, end_line \
             FROM sections ORDER BY doc_path, ordinal",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SectionRecord {
                section_id: row.get(0)?,
                doc_path: row.get(1)?,
                ordinal: row.get(2)?,
                heading_path: row.get(3)?,
                heading_level: row.get(4)?,
                parent_heading_path: row.get(5)?,
                body_text: row.get(6)?,
                first_paragraph: row.get(7)?,
                section_hash: row.get(8)?,
                heading_line: row.get::<_, i64>(9)? as usize,
                body_start_line: row.get::<_, i64>(10)? as usize,
                end_line: row.get::<_, i64>(11)? as usize,
            })
        })?;

        let mut sections = Vec::new();
        for row in rows {
            sections.push(row?);
        }
        Ok(sections)
    }

    pub fn load_all_doc_embeddings(&self) -> Result<Vec<DocumentEmbeddingRecord>> {
        let conn = self.open()?;
        let mut stmt =
            conn.prepare("SELECT doc_path, embedding_json FROM doc_embeddings ORDER BY doc_path")?;
        let rows = stmt.query_map([], |row| {
            let embedding_json: String = row.get(1)?;
            let embedding: Vec<f32> = serde_json::from_str(&embedding_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;

            Ok(DocumentEmbeddingRecord {
                doc_path: row.get(0)?,
                embedding,
            })
        })?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    pub fn load_all_graph_nodes(&self) -> Result<Vec<GraphNodeRecord>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT node_id, node_type, ref_path, ref_section, label, payload_json \
             FROM graph_nodes ORDER BY node_type, ref_path, ref_section",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(GraphNodeRecord {
                node_id: row.get(0)?,
                node_type: row.get(1)?,
                ref_path: row.get(2)?,
                ref_section: row.get(3)?,
                label: row.get(4)?,
                payload_json: row.get(5)?,
            })
        })?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }
        Ok(nodes)
    }

    pub fn load_all_graph_edges(&self) -> Result<Vec<GraphEdgeRecord>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT edge_id, src_node_id, dst_node_id, edge_type, weight, evidence_json, graph_fingerprint \
             FROM graph_edges ORDER BY src_node_id, edge_type, dst_node_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(GraphEdgeRecord {
                edge_id: row.get(0)?,
                src_node_id: row.get(1)?,
                dst_node_id: row.get(2)?,
                edge_type: row.get(3)?,
                weight: row.get(4)?,
                evidence_json: row.get(5)?,
                graph_fingerprint: row.get(6)?,
            })
        })?;

        let mut edges = Vec::new();
        for row in rows {
            edges.push(row?);
        }
        Ok(edges)
    }

    pub fn load_sections_by_doc(&self, doc_path: &str) -> Result<Vec<SectionRecord>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT section_id, doc_path, ordinal, heading_path, heading_level, parent_heading_path, body_text, first_paragraph, section_hash, heading_line, body_start_line, end_line \
             FROM sections WHERE doc_path = ?1 ORDER BY ordinal",
        )?;
        let rows = stmt.query_map(params![doc_path], |row| {
            Ok(SectionRecord {
                section_id: row.get(0)?,
                doc_path: row.get(1)?,
                ordinal: row.get(2)?,
                heading_path: row.get(3)?,
                heading_level: row.get(4)?,
                parent_heading_path: row.get(5)?,
                body_text: row.get(6)?,
                first_paragraph: row.get(7)?,
                section_hash: row.get(8)?,
                heading_line: row.get::<_, i64>(9)? as usize,
                body_start_line: row.get::<_, i64>(10)? as usize,
                end_line: row.get::<_, i64>(11)? as usize,
            })
        })?;

        let mut sections = Vec::new();
        for row in rows {
            sections.push(row?);
        }
        Ok(sections)
    }

    pub fn load_section(
        &self,
        doc_path: &str,
        heading_path: &str,
    ) -> Result<Option<SectionRecord>> {
        let conn = self.open()?;
        conn.query_row(
            "SELECT section_id, doc_path, ordinal, heading_path, heading_level, parent_heading_path, body_text, first_paragraph, section_hash, heading_line, body_start_line, end_line \
             FROM sections WHERE doc_path = ?1 AND heading_path = ?2 LIMIT 1",
            params![doc_path, heading_path],
            |row| {
                Ok(SectionRecord {
                    section_id: row.get(0)?,
                    doc_path: row.get(1)?,
                    ordinal: row.get(2)?,
                    heading_path: row.get(3)?,
                    heading_level: row.get(4)?,
                    parent_heading_path: row.get(5)?,
                    body_text: row.get(6)?,
                    first_paragraph: row.get(7)?,
                    section_hash: row.get(8)?,
                    heading_line: row.get::<_, i64>(9)? as usize,
                    body_start_line: row.get::<_, i64>(10)? as usize,
                    end_line: row.get::<_, i64>(11)? as usize,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn total_chunks(&self) -> Result<usize> {
        let conn = self.open()?;
        let count = conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(count as usize)
    }

    pub fn total_sections(&self) -> Result<usize> {
        let conn = self.open()?;
        let count = conn.query_row("SELECT COUNT(*) FROM sections", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(count as usize)
    }

    pub fn load_top_dir_buckets(&self, limit: usize) -> Result<Vec<OverviewBucket>> {
        self.load_top_graph_buckets("dir", limit)
    }

    pub fn load_top_tag_buckets(&self, limit: usize) -> Result<Vec<OverviewBucket>> {
        self.load_top_graph_buckets("tag", limit)
    }

    fn open(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)
            .with_context(|| format!("failed to open sqlite {}", self.db_path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(conn)
    }

    fn load_top_graph_buckets(&self, node_type: &str, limit: usize) -> Result<Vec<OverviewBucket>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT ref_path, label, payload_json FROM graph_nodes WHERE node_type = ?1",
        )?;
        let rows = stmt.query_map(params![node_type], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        let mut buckets = Vec::new();
        for row in rows {
            let (ref_path, label, payload_json) = row?;
            let payload: serde_json::Value = serde_json::from_str(&payload_json)?;
            let count = payload
                .get("doc_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as usize;
            let key = if node_type == "dir" { ref_path } else { label };
            if key.is_empty() || key == "." {
                continue;
            }
            buckets.push(OverviewBucket { key, count });
        }

        buckets.sort_by(|lhs, rhs| {
            rhs.count
                .cmp(&lhs.count)
                .then_with(|| lhs.key.cmp(&rhs.key))
        });
        buckets.truncate(limit);
        Ok(buckets)
    }

    fn ensure_document_columns(&self, conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(documents)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut has_embedding_fingerprint = false;
        for row in rows {
            if row? == "embedding_fingerprint" {
                has_embedding_fingerprint = true;
                break;
            }
        }
        if !has_embedding_fingerprint {
            conn.execute(
                "ALTER TABLE documents ADD COLUMN embedding_fingerprint TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_section_columns(&self, conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(sections)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let columns = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|name| name == "heading_line") {
            conn.execute(
                "ALTER TABLE sections ADD COLUMN heading_line INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "body_start_line") {
            conn.execute(
                "ALTER TABLE sections ADD COLUMN body_start_line INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "end_line") {
            conn.execute(
                "ALTER TABLE sections ADD COLUMN end_line INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        Ok(())
    }
    fn ensure_chunk_columns(&self, conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(chunks)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let columns = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|name| name == "section_id") {
            conn.execute(
                "ALTER TABLE chunks ADD COLUMN section_id TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        if !columns
            .iter()
            .any(|name| name == "chunk_ordinal_in_section")
        {
            conn.execute(
                "ALTER TABLE chunks ADD COLUMN chunk_ordinal_in_section INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "start_line") {
            conn.execute(
                "ALTER TABLE chunks ADD COLUMN start_line INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "end_line") {
            conn.execute(
                "ALTER TABLE chunks ADD COLUMN end_line INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        Ok(())
    }
}
