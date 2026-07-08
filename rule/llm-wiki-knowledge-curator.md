---
description: Retrieve llm_wiki knowledge before responses and task work, then write back durable corrections and reusable findings.
alwaysApply: true
globs:
  - "**/*"
---

# llm_wiki knowledge workflow

- Before any response, retrieval, planning, implementation, or professional knowledge explanation, query the llm_wiki MCP index instead of relying on memory; terminology and knowledge questions are the highest-priority case.
- Prefer `search_sections` for concept or terminology discovery, `search_knowledge` for evidence-level chunks, `related` for neighboring notes, and `get_document_outline` to choose where to edit.
- Treat Markdown notes as the only source of truth; graph, SQLite, embeddings, and search results are derived layers.
- When a task yields durable knowledge, update the appropriate Markdown note instead of leaving it only in chat or code.
- If an existing note is wrong, stale, or contradictory, correct or reconcile it in place; do not leave competing versions behind.
- Prefer extending an existing note over creating a new one; avoid ephemeral, private, or duplicate content.
- Keep edits concise and evidence-backed; preserve valid frontmatter when present.
- Use `reindex_all` only as an explicit fallback; normal refresh should come from the watcher.
