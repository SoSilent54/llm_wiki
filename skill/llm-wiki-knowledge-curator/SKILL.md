---
name: llm-wiki-knowledge-curator
description: >-
  Use when starting engineering work that may rely on reusable technical knowledge
  covered by this llm_wiki repository, or when the task discovers durable new
  knowledge worth preserving. Before substantial work, proactively query the
  llm_wiki MCP index for existing knowledge. After learning validated,
  reusable facts, update the Markdown knowledge base and keep it consistent as
  the source of truth.
triggers:
  - llm_wiki
  - knowledge accumulation
  - wiki retrieval before task
  - update knowledge base after task
  - capture reusable engineering knowledge
---

# llm_wiki Knowledge Curator

## Purpose

This skill enforces a two-part loop for knowledge work around `llm_wiki`:

1. **Retrieve first** — before substantial implementation, debugging, or design,
   query the existing wiki through MCP instead of relying on memory.
2. **Accumulate after learning** — when the task produces durable knowledge,
   write it back into the Markdown knowledge base so the next agent can start
   from a better state.

Markdown files remain the only source of truth. Graph, SQLite, embeddings, and
MCP search are derived layers.

The exact MCP tool names are `search_knowledge`, `search_sections`, `related`,
`get_document_outline`, `get_metadata_template`, `check_metadata`, and
`reindex_all`. Do not confuse them with similarly named CLI commands such as
`search` or `lint-metadata`.

## Mandatory Workflow

### 1. Retrieve before starting the task

Before substantial work, proactively query the wiki with MCP:

1. Use `search_sections` to find likely concept summaries.
2. Use `search_knowledge` to pull evidence-level chunks.
3. Use `related` to expand semantic or explicit neighbors around the best hit.
4. Use `get_document_outline` when you need section-level placement or editing context.
5. If the task is broad or the first searches are weak, iterate the retrieval until the knowledge surface is clear.

Do not skip retrieval just because the topic feels familiar.

## Retrieval Heuristics

Prefer this progression:

- concept / architecture question -> `search_sections`
- exact mechanism / command / evidence -> `search_knowledge`
- neighboring topics / alternatives / related notes -> `related`
- choose where to update -> `get_document_outline`

Use multiple short queries rather than one overloaded query when the first result set is noisy.

## 2. Accumulate durable knowledge after learning

When the task uncovers validated, reusable knowledge, update the wiki.

### Write back when the new knowledge is:

- a root-cause explanation
- a corrected mental model
- a precise command or workflow that was verified
- an implementation detail that future agents will likely need again
- a comparison between similar systems, algorithms, or versions
- a caveat, boundary, or failure mode that avoids repeated mistakes
- a measured runtime / build / deployment fact that materially changes decisions

### Do not write back when the information is only:

- temporary scratch work
- user-private context unrelated to the shared knowledge base
- a one-off task log with no future reuse value
- duplicate content already present in the appropriate note

## Update Policy

- Prefer extending an existing note when a natural home already exists.
- Create a new note only when the knowledge does not fit an existing document.
- Keep the note focused, technical, and durable.
- Preserve the repository's Markdown-first organization and existing naming style.
- If the target document uses frontmatter, keep it valid. Use `get_metadata_template` / `check_metadata` when needed; the similarly named CLI flow is `lint-metadata`.

## Editing Policy

- Use MCP locators to find the right document and section first.
- Then read and edit the Markdown source directly.
- Keep structure and wording concise; avoid turning notes into raw transcripts.
- Record evidence-backed conclusions, not speculation.
- Prefer updating the smallest correct section instead of appending a loose dump.

## Reindex Policy

- Normal background refresh should come from the configured watcher.
- Use `reindex_all` only as an explicit fallback or when a fresh rebuild is required.

## Acceptance Checklist

Before finishing a task covered by this skill, check:

- Did I query the wiki before substantive work?
- Did I inspect related existing notes before writing a new one?
- Did I capture durable new knowledge discovered during the task?
- Did I avoid writing ephemeral or duplicate material?
- If I edited Markdown, is the source-of-truth note now better for the next agent?
