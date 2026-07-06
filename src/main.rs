use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use llm_wiki::{config::AppConfig, markdown, mcp, model::MetadataPatch, service::KnowledgeService};

#[derive(Debug, Parser)]
#[command(name = "llm-wiki")]
#[command(about = "Markdown 知识库索引与 MCP 服务")]
struct Cli {
    /// 配置文件路径
    #[arg(long, default_value = "config/llm_wiki.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 执行一次全量扫描；仅重建变更文件
    Index,
    /// 搜索知识库
    Search {
        #[arg(long)]
        query: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// 搜索 section 摘要层
    SearchSections {
        #[arg(long)]
        query: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// 返回知识库总览
    LibraryOverview,
    /// 列出知识树中的目录与文档节点
    ListDocuments {
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        depth: Option<usize>,
    },
    /// 从图谱层扩展相关节点
    Related {
        #[arg(long)]
        path: String,
        #[arg(long)]
        heading_path: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long = "edge-type")]
        edge_types: Vec<String>,
    },
    /// 读取知识库原始 Markdown
    Read {
        #[arg(long)]
        path: String,
    },
    /// 读取文档 section 大纲
    Outline {
        #[arg(long)]
        path: String,
    },
    /// 读取单个 section 正文
    ReadSection {
        #[arg(long)]
        path: String,
        #[arg(long)]
        heading_path: Option<String>,
    },
    /// 检查 Wiki frontmatter 元数据是否符合规范；可选单文档模式
    LintMetadata {
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = false)]
        strict: bool,
    },
    /// 输出通用模板，或为指定文档推断 frontmatter 模板
    MetadataTemplate {
        #[arg(long)]
        path: Option<String>,
    },
    /// 为单文档或整个知识库补写 metadata 模板
    ApplyMetadataTemplate {
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = false)]
        overwrite: bool,
    },
    /// 用 JSON patch 写入单文档 metadata
    WriteMetadata {
        #[arg(long)]
        path: String,
        #[arg(long)]
        patch_json: String,
    },
    /// 通过 stdio 启动 MCP 服务
    ServeMcp,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::MetadataTemplate { path } => {
            if let Some(path) = path {
                let config = AppConfig::load(&cli.config)?;
                let service = KnowledgeService::new_without_embedder(config)?;
                let result = service.metadata_template(&path)?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{}", markdown::metadata_template());
            }
        }
        Command::LintMetadata { path, strict } => {
            let config = AppConfig::load(&cli.config)?;
            if let Some(path) = path {
                let service = KnowledgeService::new_without_embedder(config)?;
                let result = service.check_metadata(&path)?;
                println!("{}", serde_json::to_string_pretty(&result)?);
                if strict && !result.metadata_valid {
                    bail!("metadata lint failed for {}", result.path);
                }
            } else {
                let report = markdown::lint_metadata_tree(&config)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                if strict && report.error_count > 0 {
                    bail!("metadata lint failed with {} errors", report.error_count);
                }
            }
        }
        Command::ApplyMetadataTemplate { path, overwrite } => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new_without_embedder(config)?;
            let result = service.apply_metadata_template(path.as_deref(), overwrite)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::WriteMetadata { path, patch_json } => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new_without_embedder(config)?;
            let patch: MetadataPatch = serde_json::from_str(&patch_json)?;
            let result = service.write_metadata(&path, &patch)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Index => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new(config)?;
            let stats = service.reindex_all()?;
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        Command::Search { query, limit } => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new(config)?;
            let result = service.search(&query, limit)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::SearchSections { query, limit } => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new(config)?;
            let result = service.search_sections(&query, limit)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::LibraryOverview => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new_without_embedder(config)?;
            let result = service.library_overview()?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::ListDocuments { prefix, depth } => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new_without_embedder(config)?;
            let result = service.list_documents(prefix.as_deref(), depth)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Related {
            path,
            heading_path,
            limit,
            edge_types,
        } => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new_without_embedder(config)?;
            let edge_types = (!edge_types.is_empty()).then_some(edge_types.as_slice());
            let result = service.related(&path, heading_path.as_deref(), limit, edge_types)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Read { path } => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new_without_embedder(config)?;
            let result = service.read_document(&path)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Outline { path } => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new_without_embedder(config)?;
            let result = service.document_outline(&path)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::ReadSection { path, heading_path } => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new_without_embedder(config)?;
            let result = service.read_section(&path, heading_path.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::ServeMcp => {
            let config = AppConfig::load(&cli.config)?;
            let service = KnowledgeService::new(config)?;
            mcp::serve_stdio(service).await?;
        }
    }

    Ok(())
}
