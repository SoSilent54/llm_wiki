# llm-wiki MCP 调用接口表

本文档对应当前 `src/mcp.rs` 暴露的 stdio MCP 接口，面向直接接入 MCP 的 agent / client。

## 1. 传输与调用约定

- 启动命令：`llm-wiki --config <config-path> serve-mcp`
- 传输方式：stdio
- framing：newline-delimited JSON（`rmcp 2.1`），不是 `Content-Length`
- 常用 MCP 方法：`initialize`、`tools/list`、`tools/call`
- 工具返回同时包含：
  - `content[0].text`：JSON 字符串
  - `structuredContent`：结构化 JSON
- 集成侧建议优先消费 `structuredContent`
- 所有 `path`、`prefix` 均相对 `knowledge_root`
- `heading_path` 使用 `父标题 > 子标题` 形式，例如：`ROS1 > ROSCONSOLE`

## 2. 推荐调用路径

1. `library_overview`：先看知识库规模与主题分布
2. `list_documents`：按目录浏览文档树
3. `search_sections` / `search_knowledge` / `related`：定位候选知识点
4. `get_document_outline`：拿 section 级 locator
5. 根据 `anchor.path + anchor.span` 直接读取或编辑 Markdown 源文件
6. `check_metadata` / `get_metadata_template`：辅助 frontmatter 校验与补写
7. `reindex_all`：仅作为 fallback/admin 工具

## 3. 通用定位结构

### 3.1 `LineSpan`

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `start_line` | `usize` | 1-based 起始行 |
| `end_line` | `usize` | 1-based 结束行，闭区间 |

### 3.2 `SourceAnchor`

`search_*`、`related`、`get_document_outline`、`read_section` 等接口会返回 locator-first 的 `anchor`：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `kind` | `string` | `document` / `section` / `evidence` |
| `path` | `string` | 相对 `knowledge_root` 的 Markdown 路径 |
| `span` | `LineSpan \| null` | 可直接映射到 `read(path:start-end)` 的源码范围 |
| `heading_path` | `string \| null` | section 标题路径 |
| `heading_level` | `i64 \| null` | Markdown heading level |
| `section_ordinal` | `i64 \| null` | section 在文档中的序号 |
| `chunk_ordinal` | `i64 \| null` | evidence chunk 在 section 中的序号 |

## 4. `write_metadata.patch` 字段

`write_metadata` 的 `patch` 为部分更新，当前支持以下字段：

| 字段 | 类型 |
| --- | --- |
| `title` | `string` |
| `tags` | `string[]` |
| `aliases` | `string[]` |
| `related` | `string[]` |
| `source_type` | `string` |
| `source_ref` | `string` |
| `status` | `string` |
| `domain` | `string` |
| `keywords` | `string[]` |
| `updated_by` | `string` |
| `updated_at` | `string` |
| `review_priority` | `string` |

## 5. MCP 工具总表

| Tool | 分类 | 参数 | 主要返回字段 | 说明 |
| --- | --- | --- | --- | --- |
| `search_knowledge` | Locate | `query`, `limit?` | `query`, `total_chunks`, `hits[{ doc_path, heading_path, score, text, anchor }]` | chunk/evidence 级召回；`anchor.kind = evidence` |
| `search_sections` | Locate | `query`, `limit?` | `query`, `total_sections`, `hits[{ doc_path, heading_path, score, first_paragraph, anchor }]` | section 摘要层召回；推荐先于整文阅读 |
| `library_overview` | Discovery | 无 | `doc_count`, `section_count`, `chunk_count`, `top_dirs[]`, `top_tags[]` | 返回知识库总览 |
| `list_documents` | Discovery | `prefix?`, `depth?` | `prefix`, `depth`, `nodes[{ path, node_type, title, child_count, tag_sample }]` | 按目录层次浏览文档树 |
| `related` | Locate | `path`, `heading_path?`, `limit?`, `edge_types?` | `source_path`, `source_heading_path`, `hits[{ target_type, target_path, target_heading, edge_type, weight, why, anchor? }]` | 从 graph 边扩展相近文档/章节 |
| `read_document` | Compatibility | `path` | `path`, `content` | 兼容回退接口；返回整篇 Markdown |
| `get_document_outline` | Discovery | `path` | `path`, `title`, `sections[{ ordinal, heading_path, heading_level, parent_heading_path, first_paragraph, anchor }]` | section 大纲入口；带行号 locator |
| `read_section` | Compatibility | `path`, `heading_path?` | `path`, `heading_path`, `heading_level`, `ordinal`, `content`, `previous_heading_path`, `next_heading_path`, `anchor` | 兼容回退接口；按 section 读正文 |
| `get_metadata_template` | Metadata Assist | `path` | `path`, `has_frontmatter`, `metadata`, `frontmatter`, `frontmatter_span`, `insert_before_line` | 为单文档推断 frontmatter 模板 |
| `check_metadata` | Metadata Validate | `path` | `path`, `has_frontmatter`, `metadata_valid`, `parse_error`, `metadata?`, `issues[]`, `frontmatter_span`, `insert_before_line`, `error_count`, `warning_count` | 校验 frontmatter 语法与规范，并给出定位信息 |
| `apply_metadata_template` | Metadata Write | `path?`, `overwrite?` | `scanned_docs`, `updated_docs`, `skipped_docs`, `failed_docs`, `documents[{ path, action, message }]` | 直接改写 Markdown 文件；可对单文档或整库补模板 |
| `write_metadata` | Metadata Write | `path`, `patch` | `path`, `action`, `metadata_valid`, `metadata`, `frontmatter`, `issues[]`, `frontmatter_span`, `insert_before_line`, `error_count`, `warning_count` | 直接改写单文档 frontmatter |
| `reindex_all` | Admin | 无 | `scanned_docs`, `updated_docs`, `skipped_docs`, `deleted_docs`, `total_chunks` | fallback/admin 工具；增量重扫整个知识树 |

## 6. 建议的 client 行为

- 优先走 locator-first：先拿 `anchor`，再直接读/改 Markdown 源文件
- `read_document` / `read_section` 更适合作为兼容入口，不应替代源文件编辑路径
- `apply_metadata_template` / `write_metadata` 会直接改文件；如果 agent 已有文件编辑能力，优先把它们视为辅助工具
- `reindex_all` 不应作为正常主路径；后续应由 watcher 自动增量刷新接管

## 7. `tools/call` 请求示例

### 7.1 `search_sections`

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "search_sections",
    "arguments": {
      "query": "rosconsole",
      "limit": 3
    }
  }
}
```

### 7.2 `check_metadata`

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "tools/call",
  "params": {
    "name": "check_metadata",
    "arguments": {
      "path": "System/ROS.md"
    }
  }
}
```
