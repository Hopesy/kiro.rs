# kiro-rs

一个用 Rust 编写的 Anthropic Claude API 兼容代理服务，将 Anthropic API 请求转换为 Kiro API 请求。

## 免责声明

本项目仅供研究使用，Use at your own risk。
本项目与 AWS / KIRO / Anthropic / Claude 等官方无关，也不代表官方立场。

## 适合什么场景

- 本地自托管 Claude 兼容代理
- 多账号 RT 轮换 / 自动刷新
- Render Free + Git 数据仓库持久化
- Windows 本地便携版长期运行

## 核心特性

- Anthropic API 兼容，支持 `/v1/messages`
- SSE 流式响应
- 多凭据故障转移与负载均衡
- RT 自动刷新并自动回写
- Admin API + Admin UI
- 支持 Render Free 外部 Git 持久化
- 支持本地数据目录持久化

---

## 快速开始

### 方式 1：直接下载 Release

如果你不想本地编译，直接去 Release 下载：

- Windows: `kiro-rs-v2026.3.6-windows-x64.exe`
- Release 页：<https://github.com/Hopesy/kiro.rs/releases>

### 方式 2：本地编译

先构建管理前端：

```bash
cd admin-ui
pnpm install
pnpm build
```

再编译 Rust：

```bash
cargo build --release
```

### 最小配置

`config.json`

```json
{
  "host": "127.0.0.1",
  "port": 8990,
  "apiKey": "sk-kiro-rs-your-public-key",
  "region": "us-east-1"
}
```

`credentials.json`

Social：

```json
{
  "refreshToken": "你的刷新token",
  "expiresAt": "2025-12-31T02:32:45.144Z",
  "authMethod": "social"
}
```

IdC：

```json
{
  "refreshToken": "你的刷新token",
  "expiresAt": "2025-12-31T02:32:45.144Z",
  "authMethod": "idc",
  "clientId": "你的clientId",
  "clientSecret": "你的clientSecret"
}
```

### 启动

```bash
./target/release/kiro-rs
```

或显式指定文件：

```bash
./target/release/kiro-rs -c /path/to/config.json --credentials /path/to/credentials.json
```

### 验证

```bash
curl http://127.0.0.1:8990/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-kiro-rs-your-public-key" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 1024,
    "stream": true,
    "messages": [
      {"role": "user", "content": "Hello, Claude!"}
    ]
  }'
```

---

## Render Free 部署流程

这是当前最推荐的部署方式：

- 服务跑在 Render Free
- 配置和账号 RT 持久化到单独 Git 数据仓库
- Render 重启后数据不丢

### 一、准备两个仓库

1. **代码仓库**：`Hopesy/kiro.rs`
2. **数据仓库**：你自己的私有仓库，例如：`Hopesy/kiro-rs-state`

数据仓库专门保存运行状态，不要和代码仓库混用。

### 二、数据仓库结构

固定结构：

```text
config/
└── config.json

auths/
├── credential-000001.json
├── credential-000002.json
└── ...
```

说明：

- `config/config.json`：服务配置源
- `auths/*.json`：一个账号一个 RT 文件
- 运行时聚合文件 `credentials.json` 只是临时文件，不需要持久化

### 三、创建 GitHub Token

需要一个对**数据仓库**有写权限的 GitHub PAT，用来让 Render 容器自动：

- clone 数据仓库
- commit 配置变化
- push 最新 RT

### 四、在 Render 创建 Blueprint 服务

仓库根目录已经提供 `render.yaml`，直接走 Blueprint：

1. 打开 Render Dashboard
2. 点击 `New`
3. 选择 `Blueprint`
4. 连接代码仓库 `Hopesy/kiro.rs`
5. Render 会自动读取 `render.yaml`

### 五、Render 环境变量怎么填

#### 必填

```text
GIT_STORAGE_REPO_URL=https://github.com/Hopesy/kiro-rs-state.git
GIT_STORAGE_AUTH_TOKEN=你的github_pat
PUBLIC_API_KEY=你的对外API密钥
```

#### 可选

```text
ADMIN_API_KEY=你的管理面板密钥
KIRO_REGION=us-east-1
```

#### 不要填

```text
PORT
GIT_STORAGE_BRANCH
GIT_STORAGE_LOCAL_DIR
GIT_STORAGE_CONFIG_PATH
GIT_STORAGE_CREDENTIALS_DIR
GIT_STORAGE_AUTHOR_NAME
GIT_STORAGE_AUTHOR_EMAIL
KIRO_REFRESH_TOKEN
```

### 六、首次启动行为

如果数据仓库里还没有：

```text
config/config.json
```

服务会自动用这些变量生成首份配置：

- `PUBLIC_API_KEY`
- `ADMIN_API_KEY`
- `KIRO_REGION`

如果数据仓库已经有 `config/config.json`，则直接读取已有配置。

### 七、后续如何持久化

以下操作都会自动同步回数据仓库：

- RT 自动刷新
- 管理面板上传单账号 RT
- 管理面板删除单账号 RT
- 配置修改（如负载均衡模式）

### 八、Render 端口说明

不需要手填 `PORT`。

Render 会自动注入 `PORT`，服务会自动监听：

```text
0.0.0.0:$PORT
```

### 九、当前 Render 镜像

当前 Blueprint 默认镜像：

```text
ghcr.io/hopesy/kiro-rs:v2026.3.6
```

---

## 本地便携版持久化

当满足以下条件时：

- **未显式传入** `-c`
- **未显式传入** `--credentials`
- **未设置** `GIT_STORAGE_REPO_URL`

程序会自动切到本地数据目录模式。

Windows 默认目录：

```text
%LocalAppData%\kiro-rs\
├── config\
│   └── config.json
├── auths\
│   ├── credential-000001.json
│   ├── credential-000002.json
│   └── ...
└── runtime\
    └── credentials.json
```

同步规则：

- RT 自动刷新 -> 回写 `auths/*.json`
- 上传单账号 RT -> 回写 `auths/*.json`
- 删除单账号 RT -> 删除对应 `auths/*.json`
- 配置修改 -> 回写 `config/config.json`

---

## 配置说明

### `config.json` 常用字段

| 字段 | 说明 |
|---|---|
| `host` | 监听地址，本地默认 `127.0.0.1` |
| `port` | 监听端口，本地默认 `8080` |
| `apiKey` | 对外 API Key，客户端调用本服务时使用 |
| `adminApiKey` | 管理面板密钥，配置后启用 `/admin` |
| `region` | 默认区域，默认 `us-east-1` |
| `authRegion` | RT 刷新区域 |
| `apiRegion` | API 请求区域 |
| `loadBalancingMode` | `priority` 或 `balanced` |
| `proxyUrl` | 全局代理 |
| `tlsBackend` | `rustls` 或 `native-tls` |
| `extractThinking` | 是否提取非流式 thinking |
| `defaultEndpoint` | 默认端点，当前默认 `ide` |

完整示例可参考：

- `config.example.json`

### `credentials.json` 支持格式

支持两种：

1. 单对象
2. 数组（多账号）

多账号数组示例：

```json
[
  {
    "refreshToken": "第一个账号的RT",
    "expiresAt": "2025-12-31T02:32:45.144Z",
    "authMethod": "social",
    "priority": 0
  },
  {
    "refreshToken": "第二个账号的RT",
    "expiresAt": "2025-12-31T02:32:45.144Z",
    "authMethod": "idc",
    "clientId": "xxxxxxxxx",
    "clientSecret": "xxxxxxxxx",
    "priority": 1
  }
]
```

### Region 优先级

**Auth Region**：

```text
凭据.authRegion > 凭据.region > config.authRegion > config.region
```

**API Region**：

```text
凭据.apiRegion > config.apiRegion > config.region
```

### 代理优先级

```text
凭据.proxyUrl > config.proxyUrl > 无代理
```

特殊值：

```text
proxyUrl=direct
```

表示该凭据显式直连，不走代理。

---

## API 端点

### 标准端点

| 端点 | 方法 | 说明 |
|---|---|---|
| `/v1/models` | GET | 获取可用模型 |
| `/v1/messages` | POST | 发送消息 |
| `/v1/messages/count_tokens` | POST | 估算 token |

### Claude Code 兼容端点

| 端点 | 方法 | 说明 |
|---|---|---|
| `/cc/v1/messages` | POST | 缓冲模式消息接口 |
| `/cc/v1/messages/count_tokens` | POST | 估算 token |

### 模型映射

| Anthropic 模型 | Kiro 模型 |
|---|---|
| `*sonnet*` | `claude-sonnet-4.5` |
| `*opus*`（含 4.5 / 4-5） | `claude-opus-4.5` |
| `*opus*`（其他） | `claude-opus-4.6` |
| `*haiku*` | `claude-haiku-4.5` |

---

## Admin

配置了非空 `adminApiKey` 后，会启用：

- `GET /admin`
- `GET /api/admin/credentials`
- `POST /api/admin/credentials`
- `DELETE /api/admin/credentials/:id`
- `POST /api/admin/credentials/:id/disabled`
- `POST /api/admin/credentials/:id/priority`
- `POST /api/admin/credentials/:id/reset`
- `GET /api/admin/credentials/:id/balance`

---

## Docker

也可以本地用 Docker 跑：

```bash
docker-compose up
```

如果你走传统文件模式，需要把 `config.json` 和 `credentials.json` 挂进容器。
如果你走 Render / Git 持久化模式，则不需要这样挂载。

---

## 注意事项

1. 请妥善保管你的 RT 和 API Key
2. 多账号模式下，刷新后的 RT 会自动写回持久化后端
3. 如果代理环境下请求异常，可尝试把 `tlsBackend` 改成 `native-tls`
4. 当 `tools` 里只有 `web_search` 时，会走内置 WebSearch 转换逻辑

## 技术栈

- Axum
- Tokio
- Reqwest
- Serde
- tracing
- Clap

## License

MIT

## 致谢

- [kiro2api](https://github.com/caidaoli/kiro2api)
- [proxycast](https://github.com/aiclientproxy/proxycast)
