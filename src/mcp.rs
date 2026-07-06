use anyhow::Result;
use rmcp::{
    Json, ServiceExt, handler::server::wrapper::Parameters, tool, tool_router, transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{
    model::{
        DocumentOutlineResponse, IndexStats, LibraryOverviewResponse, ListDocumentsResponse,
        MetadataCheckResponse, MetadataTemplateResponse, RelatedResponse, SearchResponse,
        SectionSearchResponse,
    },
    service::KnowledgeService,
};

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchParams {
    /// 查询文本
    query: String,
    /// 返回结果数量；未传时使用配置默认值
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PathParams {
    /// 相对于 knowledge_root 的 Markdown 路径
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListDocumentsParams {
    /// 可选前缀目录；为空时从知识库根开始
    prefix: Option<String>,
    /// 相对 prefix 的层级深度；默认 1
    depth: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RelatedParams {
    /// 相对于 knowledge_root 的 Markdown 路径
    path: String,
    /// 可选 section heading_path；传入后按 section 扩展，否则按文档扩展
    heading_path: Option<String>,
    /// 返回结果数量；未传时使用配置默认值
    limit: Option<usize>,
    /// 可选边类型过滤；例如 related_to / tagged_with / semantic_similar_*
    edge_types: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct WikiMcpServer {
    service: KnowledgeService,
}

impl WikiMcpServer {
    pub fn new(service: KnowledgeService) -> Self {
        Self { service }
    }
}

#[tool_router(server_handler)]
impl WikiMcpServer {
    #[tool(
        description = "Search evidence chunks in the indexed Markdown knowledge base and return source locators"
    )]
    fn search_knowledge(
        &self,
        Parameters(SearchParams { query, limit }): Parameters<SearchParams>,
    ) -> Result<Json<SearchResponse>, String> {
        self.service
            .search(&query, limit)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Search section summaries in the indexed Markdown knowledge base and return source locators"
    )]
    fn search_sections(
        &self,
        Parameters(SearchParams { query, limit }): Parameters<SearchParams>,
    ) -> Result<Json<SectionSearchResponse>, String> {
        self.service
            .search_sections(&query, limit)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "Get a high-level overview of the indexed library")]
    fn library_overview(&self) -> Result<Json<LibraryOverviewResponse>, String> {
        self.service
            .library_overview()
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "List directory and document nodes from the indexed knowledge tree")]
    fn list_documents(
        &self,
        Parameters(ListDocumentsParams { prefix, depth }): Parameters<ListDocumentsParams>,
    ) -> Result<Json<ListDocumentsResponse>, String> {
        self.service
            .list_documents(prefix.as_deref(), depth)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "Expand related graph nodes from a document or section")]
    fn related(
        &self,
        Parameters(RelatedParams {
            path,
            heading_path,
            limit,
            edge_types,
        }): Parameters<RelatedParams>,
    ) -> Result<Json<RelatedResponse>, String> {
        self.service
            .related(&path, heading_path.as_deref(), limit, edge_types.as_deref())
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Get the stored section outline of a Markdown document with line-based locators"
    )]
    fn get_document_outline(
        &self,
        Parameters(PathParams { path }): Parameters<PathParams>,
    ) -> Result<Json<DocumentOutlineResponse>, String> {
        self.service
            .document_outline(&path)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "Infer a metadata frontmatter template for one Markdown document")]
    fn get_metadata_template(
        &self,
        Parameters(PathParams { path }): Parameters<PathParams>,
    ) -> Result<Json<MetadataTemplateResponse>, String> {
        self.service
            .metadata_template(&path)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        description = "Check one Markdown document's frontmatter syntax and lint issues with source spans"
    )]
    fn check_metadata(
        &self,
        Parameters(PathParams { path }): Parameters<PathParams>,
    ) -> Result<Json<MetadataCheckResponse>, String> {
        self.service
            .check_metadata(&path)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(description = "Admin tool: reindex the full Markdown tree incrementally")]
    fn reindex_all(&self) -> Result<Json<IndexStats>, String> {
        self.service
            .reindex_all()
            .map(Json)
            .map_err(|err| err.to_string())
    }
}

pub async fn serve_stdio(service: KnowledgeService) -> Result<()> {
    let server = WikiMcpServer::new(service).serve(stdio()).await?;
    server.waiting().await?;
    Ok(())
}
