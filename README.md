# llm-wiki

`llm-wiki` 是一个面向 agent / MCP 的本地 Markdown 知识库编译器。

它把外部 `knowledge_root/**/*.md` 作为唯一 source of truth，编译出：

- 文档 / section / evidence(chunk) 分层索引
- doc / section 向量检索层
- 基础知识图谱层
- 基于 stdio 的 MCP server

当前默认运行态使用 `fastembed + 动态 ONNX Runtime`；如果你只想先跑通，也可以切回 `hashing` 后端，完全不依赖 ORT。

## 1. 主要能力

- 扫描单一 Markdown 知识树，跳过隐藏文件和 `.obsidian/`
- 文档级增量索引：按 `file_hash + embedding_fingerprint` 跳过未变更文档
- section / chunk 行号落库，返回 `path + line span` 风格 locator
- chunk 级召回：`search`
- section 摘要级召回：`search-sections`
- 图谱浏览：`library-overview` / `list-documents` / `related`
- metadata 辅助：`metadata-template` / `lint-metadata`
- stdio MCP server：供 OMP 或其他 MCP client 直接接入

## 2. 仓库内容

```text
.
├── src/                               # Rust 源码
├── config/llm_wiki.template.toml      # 配置模板
├── docs/mcp_interface.md              # MCP 接口参考
├── model/fetch_fastembed_model.sh     # Linux / macOS 模型缓存下载脚本
├── model/fetch_fastembed_model.ps1    # Windows PowerShell 模型缓存下载脚本
├── systemd/                           # Linux systemd 示例
└── .github/workflows/                 # CI / release 工作流
```

> 本仓库不承载你的知识内容本体。
> 真实知识内容应该放在仓库外部的 Markdown 目录里，通过 `knowledge_root` 指向。

## 3. 运行前提

### 3.1 Rust / Cargo（仅源码构建）

源码构建需要：

- Rust stable toolchain
- `cargo`

本项目当前用 `cargo build --locked` / `cargo test --locked` 验证。

如果你是**直接下载 GitHub Release 里的预编译包**，运行时**不需要** Rust / Cargo。

### 3.2 fastembed 默认运行时依赖

默认配置是：

- `embedding_backend = "fastembed"`
- `fastembed_model = "AllMiniLML6V2"`

因此运行时还需要：

1. **模型缓存**
2. **ONNX Runtime 动态库**

当前开发环境（Ubuntu 20.04）实际使用并验证的是 **ONNX Runtime 1.19.2**。

结合当前依赖组合 `fastembed = 4.0.0` / `ort = 2.0.0-rc.5`，可以先把**理论适配范围**理解为 **ONNX Runtime 1.18.x ~ 1.19.x**；更新版本暂时**没有做新版适配和回归验证**。

`llm-wiki` 当前不会把 ORT 动态库打进仓库，也不会把模型本体提交进 git。

### 3.3 如果你不想先处理 ORT

可以把配置改成：

```toml
embedding_backend = "hashing"
```

这样：

- 不需要 ORT 动态库
- 不需要模型缓存
- 可以先验证索引/MCP 链路
- 检索质量会低于真实 embedding

## 4. 从源码构建

### 4.1 构建二进制

```bash
cargo build --release --locked
```

产物：

```text
target/release/llm-wiki
```

### 4.2 准备本地配置文件

复制模板：

```bash
cp config/llm_wiki.template.toml config/llm_wiki.toml
```

然后至少修改这几个字段：

```toml
knowledge_root = "/absolute/path/to/your/WIKI"
state_dir = "../.llm_wiki_state"
database_path = "../.llm_wiki_state/index.sqlite3"
embedding_backend = "fastembed"   # 或 hashing
embedding_cache_dir = "../model/fastembed"
```

### 4.3 关键配置说明

| 配置项 | 必填 | 说明 |
| --- | --- | --- |
| `knowledge_root` | 是 | 外部 Markdown 知识库绝对路径 |
| `state_dir` | 否 | 索引状态目录；相对路径相对于配置文件目录解析 |
| `database_path` | 否 | SQLite 索引库路径 |
| `embedding_backend` | 否 | `fastembed` 或 `hashing` |
| `fastembed_model` | 否 | 当前默认模型名：`AllMiniLML6V2` |
| `embedding_cache_dir` | 否 | fastembed 模型缓存目录 |
| `chunk_char_limit` | 否 | chunk 切分长度上限 |
| `search_limit` | 否 | 默认搜索结果条数 |
| `metadata_frontmatter_enabled` | 否 | 是否解析 frontmatter |
| `graph_enabled` | 否 | 是否构建图谱层 |

> 代码里会把相对路径解析到**配置文件所在目录**，不是当前 shell 目录。见 `src/config.rs`。

### 4.4 准备默认模型缓存（仅 fastembed）

Linux / macOS：

```bash
./model/fetch_fastembed_model.sh
```

Windows PowerShell：

```powershell
.\model\fetch_fastembed_model.ps1
```

脚本会在 `model/fastembed/` 下准备当前默认模型 `AllMiniLML6V2` 的缓存。

### 4.5 准备 ONNX Runtime 动态库（仅 fastembed）

运行时需要让 `fastembed` 能找到 ORT 动态库。

| 平台 | `ORT_DYLIB_PATH` 应指向 | 建议额外设置 |
| --- | --- | --- |
| Linux | `/path/to/libonnxruntime.so` | `LD_LIBRARY_PATH=/path/to/onnxruntime/dir` |
| macOS | `/path/to/libonnxruntime.dylib` | `DYLD_LIBRARY_PATH=/path/to/onnxruntime/dir` |
| Windows | `C:\path\to\onnxruntime.dll` | 把 dll 所在目录加入 `PATH` |

版本说明：

- 当前开发环境（Ubuntu 20.04）已验证：**ONNX Runtime 1.19.2**
- 当前代码分支可先按**理论适配范围 `1.18.x ~ 1.19.x`** 理解
- `1.20+` 暂时**没有做新版适配和完整回归验证**

Linux 示例：

```bash
export ORT_DYLIB_PATH=/path/to/onnxruntime/capi/libonnxruntime.so
export LD_LIBRARY_PATH=/path/to/onnxruntime/capi
```

> 注意：只要当前配置仍然使用 `fastembed`，`index`、`search`、`search-sections`、`serve-mcp` 在启动时就会初始化 embedding engine，因此不是“第一次搜索时”才需要 ORT；**MCP server 启动本身就需要**模型缓存和 ORT 动态库。

### 4.6 执行首次索引

```bash
./target/release/llm-wiki --config config/llm_wiki.toml index
```

### 4.7 启动 MCP server

```bash
./target/release/llm-wiki --config config/llm_wiki.toml serve-mcp
```

## 5. 从 GitHub Release 下载使用

当仓库接到 GitHub 后，tag 版发布会自动上传多平台预编译资产。

### 5.1 预期发布资产

当前 release workflow 会构建这些平台：

- `ubuntu20.04-x86_64-unknown-linux-gnu`
- `ubuntu20.04-aarch64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`
- `x86_64-pc-windows-msvc`
- `aarch64-pc-windows-msvc`
- `aarch64-apple-darwin`

资产命名格式：

```text
llm-wiki-<tag>-<package-id>.tar.gz
llm-wiki-<tag>-<package-id>.zip
```

其中：

- Linux / macOS：`tar.gz`
- Windows：`zip`
- `ubuntu20.04-*` 是 glibc 基线包，优先面向 Ubuntu 20 / glibc 2.31 一侧
- `*-unknown-linux-musl` 是更通用的 Linux musl 包，用来避开 glibc 版本差异；它和 Ubuntu20 glibc 包是两条不同发行线

### 5.2 Release 资产内容

每个压缩包会包含：

```text
llm-wiki-<tag>-<package-id>/
├── llm-wiki[.exe]
├── README.md
├── config/llm_wiki.template.toml
├── docs/mcp_interface.md
├── model/fetch_fastembed_model.sh
├── model/fetch_fastembed_model.ps1
└── systemd/
```

### 5.3 下载后的配置流程

直接下载 release 包运行时，**不需要安装 Rust / Cargo**。你只需要：

- 对应平台的预编译二进制
- 本地 `config/llm_wiki.toml`
- 你的 Markdown 知识库目录
- 如果使用 `fastembed`，再额外准备模型缓存和 ORT 动态库

也就是说：

- **Rust 只属于源码构建依赖**
- **ORT 才是当前 `fastembed` 路线的运行时依赖**
- `*-unknown-linux-musl` 资产如果仍要使用 `fastembed`，不要默认复用 Ubuntu/glibc 版 ORT；更稳妥的是准备 musl 兼容的 ORT，或者直接切到 `hashing`

1. 解压 release 资产
2. 复制 `config/llm_wiki.template.toml` 为本地 `config/llm_wiki.toml`
3. 修改 `knowledge_root`
4. 如果使用 `fastembed`：
   - Linux / macOS 执行 `model/fetch_fastembed_model.sh`
   - Windows 执行 `model\fetch_fastembed_model.ps1`
   - 准备 ORT 动态库，并设置环境变量
5. 运行：

```bash
./llm-wiki --config config/llm_wiki.toml index
./llm-wiki --config config/llm_wiki.toml serve-mcp
```

> Release 资产只负责分发程序与模板，不会携带你的知识库内容，也不会默认附带 ORT 动态库。

## 6. MCP / OMP 配置流程

### 6.1 通用 MCP 启动命令

```bash
llm-wiki --config /absolute/path/to/config/llm_wiki.toml serve-mcp
```

传输方式：

- stdio
- newline-delimited JSON（不是 `Content-Length` framing）

详细接口见：[`docs/mcp_interface.md`](docs/mcp_interface.md)

### 6.2 OMP 配置示例

把下面内容合并进：

```text
~/.omp/agent/mcp.json
```

示例：

```json
{
  "mcpServers": {
    "llm-wiki": {
      "type": "stdio",
      "command": "/absolute/path/to/llm-wiki",
      "args": [
        "--config",
        "/absolute/path/to/config/llm_wiki.toml",
        "serve-mcp"
      ],
      "env": {
        "ORT_DYLIB_PATH": "/absolute/path/to/onnxruntime/capi/libonnxruntime.so",
        "LD_LIBRARY_PATH": "/absolute/path/to/onnxruntime/capi"
      },
      "enabled": true,
      "timeout": 30000
    }
  }
}
```

说明：

- 如果你改用 `hashing` 后端，可以去掉 ORT 相关环境变量
- 修改 `mcp.json` 后，建议重启 OMP 会话
- 当前 MCP 契约是 locator-first：先拿 `anchor.path + anchor.span`，再直接读/改 Markdown 源文件

### 6.3 当前 MCP 工具

当前 `src/mcp.rs` 暴露 9 个工具：

- `search_knowledge`
- `search_sections`
- `library_overview`
- `list_documents`
- `related`
- `get_document_outline`
- `get_metadata_template`
- `check_metadata`
- `reindex_all`

## 7. 常用 CLI 命令

```bash
# 增量重建整个知识树
./target/release/llm-wiki --config config/llm_wiki.toml index

# chunk 级检索
./target/release/llm-wiki --config config/llm_wiki.toml search --query "rosconsole" --limit 5

# section 摘要级检索
./target/release/llm-wiki --config config/llm_wiki.toml search-sections --query "EGO Planner" --limit 5

# 看知识库概览
./target/release/llm-wiki --config config/llm_wiki.toml library-overview

# 按目录浏览索引树
./target/release/llm-wiki --config config/llm_wiki.toml list-documents --prefix ISP --depth 2

# 看文档 section 大纲
./target/release/llm-wiki --config config/llm_wiki.toml outline --path "System/ROS.md"

# 检查全库 metadata
./target/release/llm-wiki --config config/llm_wiki.toml lint-metadata

# 检查单文档 metadata
./target/release/llm-wiki --config config/llm_wiki.toml lint-metadata --path "System/ROS.md"

# 推断单文档 metadata 模板
./target/release/llm-wiki --config config/llm_wiki.toml metadata-template --path "System/ROS.md"
```

## 8. Linux systemd 示例

仓库已提供示例：

- `systemd/llm-wiki-index.service`
- `systemd/llm-wiki-mcp.service`

使用方法：

1. 根据你的安装路径替换占位符
2. 确认 `ExecStart` 中 `--config` 指向真实本地配置
3. 如果使用 `fastembed`，同时配置 ORT 环境变量

## 9. GitHub Actions 与 tag 发布

仓库会包含两个 workflow：

- `.github/workflows/ci.yml`
  - 在 `main` / `master` 的 `push` 与 `pull_request` 上执行 `cargo fmt --check` 和 `cargo test --locked`
  - 当前 verify matrix 覆盖 `ubuntu-22.04`、`ubuntu-24.04`、`ubuntu-24.04-arm`
  - 也支持 `workflow_dispatch` 手动触发
- `.github/workflows/release.yml`
  - 在 tag `v*.*.*` 上触发
  - release 前会先做 hosted verify（`ubuntu-22.04`、`ubuntu-24.04`、`ubuntu-24.04-arm`）
  - 同时再做 Ubuntu 20 容器 verify（Linux `x86_64 + arm64`）
  - release 构建矩阵产出 Ubuntu20 Linux `x86_64 + arm64`、Linux musl `x86_64 + arm64`、Windows `x86_64 + arm64`、macOS arm64
  - Linux 资产现在分成两类：`ubuntu20.04-*` glibc 基线包，以及 `*-unknown-linux-musl` 通用 musl 包
  - 也支持 `workflow_dispatch`，手动输入 tag 发布
  - 先验证，再构建多平台 release 资产
  - 自动创建 GitHub Release 并上传压缩包

### 9.1 维护者发布步骤

1. 确认工作区代码、README、workflow 都已完成
2. 如有需要，更新 `Cargo.toml` 里的版本号
3. 创建版本 tag，例如：

```bash
git tag v0.1.0
```

4. 把 commit 和 tag 推到 GitHub 仓库
5. 等待 `release.yml` 跑完
6. 在 GitHub Releases 页面检查资产是否齐全

### 9.2 当前 release workflow 的行为

```text
push main/master or pull_request
  └── CI
      └── verify
          ├── cargo fmt --check
          └── cargo test --locked

tag vX.Y.Z or workflow_dispatch(tag)
  └── Release
      ├── hosted verify
      │   ├── ubuntu-22.04
      │   ├── ubuntu-24.04
      │   └── ubuntu-24.04-arm
      ├── ubuntu20 container verify
      │   ├── Linux x86_64
      │   └── Linux arm64
      ├── build matrix
      │   ├── Ubuntu20 Linux x86_64
      │   ├── Ubuntu20 Linux arm64
      │   ├── musl Linux x86_64
      │   ├── musl Linux arm64
      │   ├── Windows x86_64
      │   ├── Windows arm64
      │   └── macOS arm64
      └── create GitHub Release
          └── upload packaged archives
```

## 10. 常见问题

### 10.1 `libonnxruntime` / `onnxruntime.dll` 加载失败

先检查：

- `ORT_DYLIB_PATH` 是否指向真实存在的动态库文件
- Linux/macOS 的库目录是否已加入 `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH`
- Windows 的 dll 目录是否已加入 `PATH`

### 10.1.1 GitHub Actions 里的 Ubuntu 20.04 怎么办

- 当前 GitHub-hosted runner 官方可用表里有 `ubuntu-22.04`、`ubuntu-24.04`、`ubuntu-26.04`，**没有 `ubuntu-20.04`**。
- 所以仓库没有直接使用 `runs-on: ubuntu-20.04`。
- 当前 release workflow 的做法是：
  - hosted verify 继续跑 `ubuntu-22.04` / `ubuntu-24.04` / `ubuntu-24.04-arm`
  - Ubuntu 20 的 Linux `x86_64` / `arm64` glibc 包继续在 `ubuntu-24.04` / `ubuntu-24.04-arm` runner 上通过 `ubuntu:20.04` container 构建与验证
  - 更通用的 Linux 发行线额外提供 `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl`

### 10.1.2 onnxruntime 动态库当前怎么处理

- 当前 release **不打包** `onnxruntime` 动态库。
- 原因不是编译做不到，而是它属于**平台/架构/安装方式强相关的运行时依赖**：
  - Linux glibc 包通常配 `libonnxruntime.so`
  - Linux musl 包更适合配 musl 兼容的 `libonnxruntime.so`
  - macOS 是 `libonnxruntime.dylib`
  - Windows 是 `onnxruntime.dll`
- 当前代码走的是 `fastembed` 的 **dynamic loading** 路线；编译期不需要链接你本机的 ORT，真正需要 ORT 的是运行 `index` / `search` / `search-sections` / `serve-mcp` 时。
- 当前开发环境（Ubuntu 20.04）实际使用并验证的是 **ONNX Runtime 1.19.2**。
- 结合当前依赖组合 `fastembed = 4.0.0` / `ort = 2.0.0-rc.5`，可先把**理论适配范围**理解为 **ONNX Runtime 1.18.x ~ 1.19.x**。
- 对 `1.20+` 以及更高版本，仓库目前**暂时没有做新版适配和回归验证**。
- 所以当前推荐策略是：
  - CI / Release 只负责构建 `llm-wiki` 二进制
  - glibc Linux / macOS / Windows 资产，用户本地通过 `ORT_DYLIB_PATH` + `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH` / `PATH` 提供匹配平台的 ORT
  - musl Linux 资产，如果继续使用 `fastembed`，优先准备 musl 兼容的 ORT；如果只是想先要更通用的 Linux 包，直接把 `embedding_backend` 改成 `hashing`

### 10.2 只想先验证索引和 MCP，不想处理模型

把配置改成：

```toml
embedding_backend = "hashing"
```

然后直接运行 `index` / `serve-mcp`。

### 10.3 为什么 release 资产里没有模型和知识库

因为：

- 模型缓存体积较大，且属于可再生依赖
- 知识库内容本身是你自己的 source of truth，不属于程序发布物

## 11. 参考文档

- MCP 接口表：[`docs/mcp_interface.md`](docs/mcp_interface.md)
- 配置模板：[`config/llm_wiki.template.toml`](config/llm_wiki.template.toml)
- Linux / macOS 模型下载脚本：[`model/fetch_fastembed_model.sh`](model/fetch_fastembed_model.sh)
- Windows 模型下载脚本：[`model/fetch_fastembed_model.ps1`](model/fetch_fastembed_model.ps1)
- Linux service 示例：[`systemd/`](systemd)
