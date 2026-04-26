# kiro-rs

一个把 Anthropic Claude API 请求转到 Kiro API 的 Rust 代理服务。

## 适合什么场景

- 本地自托管 Claude 兼容代理
- 多账号 RT 自动刷新 / 自动切换
- Render Free + Git 数据仓库持久化
- Windows 本地便携长期运行

## 核心特性

- 兼容 `/v1/messages`、`/v1/models`、`/v1/messages/count_tokens`
- 支持多凭据、自动故障转移、优先级 / 均衡负载
- RT 自动刷新，并自动回写到持久化后端
- 支持 Admin API 与 `/admin` 管理页面
- 支持 Render Free + Git 持久化
- 支持本地 `%LocalAppData%\kiro-rs` 持久化

## 快速开始

### 1. 直接下载 Release

Release：<https://github.com/Hopesy/kiro.rs/releases>

Windows 直接下载：

```text
kiro-rs-v2026.3.6-windows-x64.exe
```

### 2. 本地编译

先构建管理前端：

```bash
cd admin-ui
pnpm install
pnpm build
```

再编译：

```bash
cargo build --release
```

### 3. 最小文件模式

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

```json
{
  "refreshToken": "你的刷新token",
  "expiresAt": "2025-12-31T02:32:45.144Z",
  "authMethod": "social"
}
```

启动：

```bash
./target/release/kiro-rs
```

验证：

```bash
curl http://127.0.0.1:8990/v1/models \
  -H "x-api-key: sk-kiro-rs-your-public-key"
```

---

## Render Free 部署

这是当前最推荐的部署方式：

- 应用跑在 Render Free
- 数据放在独立 Git 数据仓库
- Render 重启后配置和 RT 不丢

### 一、准备两个仓库

1. 代码仓库：`Hopesy/kiro.rs`
2. 数据仓库：你自己的私有仓库，例如 `Hopesy/kiro-rs-state`

数据仓库固定结构：

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
- 运行时聚合 `credentials.json` 只是临时文件，不需要持久化

### 二、准备 GitHub Token

创建一个对**数据仓库**有写权限的 GitHub PAT，供 Render 容器自动：

- clone 数据仓库
- commit 配置变化
- push 最新 RT

### 三、用 Blueprint 创建 Render 服务

仓库根目录已经提供 `render.yaml`。

创建步骤：

1. 打开 Render Dashboard
2. 点击 `New`
3. 选择 `Blueprint`
4. 连接 `Hopesy/kiro.rs`
5. Render 自动读取 `render.yaml`

### 四、Render 环境变量

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

### 五、首次启动行为

如果数据仓库里没有 `config/config.json`：

- 服务会用 `PUBLIC_API_KEY`
- `ADMIN_API_KEY`
- `KIRO_REGION`

自动生成首份配置。

如果数据仓库已经有 `config/config.json`，则直接读取已有配置。

### 六、会自动同步回 Git 的内容

- RT 自动刷新
- 管理面板上传单账号 RT
- 管理面板删除单账号 RT
- 配置修改

### 七、部署后访问地址

部署成功后，访问地址就是 Render 给你的 `Public URL / Live URL`，例如：

```text
https://kiro-rs-xxxx.onrender.com
```

不要手动拼端口。直接访问：

```text
GET  https://kiro-rs-xxxx.onrender.com/v1/models
POST https://kiro-rs-xxxx.onrender.com/v1/messages
POST https://kiro-rs-xxxx.onrender.com/v1/messages/count_tokens
GET  https://kiro-rs-xxxx.onrender.com/admin
```

说明：

- 外部访问不要带 `:端口`
- Render 会自动映射到容器内 `$PORT`
- 根路径 `/` 不一定有页面
- 建议优先检查 `/v1/models` 和 `/admin`

### 八、当前默认镜像

```text
ghcr.io/hopesy/kiro-rs:v2026.3.6
```

---

## 本地便携版持久化

当满足以下条件时：

- 没有显式传 `-c`
- 没有显式传 `--credentials`
- 没有设置 `GIT_STORAGE_REPO_URL`

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

会自动同步：

- RT 自动刷新
- 上传单账号 RT
- 删除单账号 RT
- 配置修改

---

## 常用配置说明

### `config.json`

最常用字段：

| 字段 | 说明 |
|---|---|
| `host` | 监听地址 |
| `port` | 监听端口 |
| `apiKey` | 对外 API Key |
| `adminApiKey` | `/admin` 管理密钥 |
| `region` | 默认区域 |
| `authRegion` | RT 刷新区域 |
| `apiRegion` | API 请求区域 |
| `loadBalancingMode` | `priority` / `balanced` |
| `proxyUrl` | 全局代理 |
| `tlsBackend` | `rustls` / `native-tls` |
| `extractThinking` | 是否提取非流式 thinking |
| `defaultEndpoint` | 默认端点，当前默认 `ide` |

完整示例见：

```text
config.example.json
```

### `credentials.json`

支持：

1. 单对象
2. 数组（多账号）

多账号示例：

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

优先级规则：

```text
Auth Region: 凭据.authRegion > 凭据.region > config.authRegion > config.region
API Region : 凭据.apiRegion > config.apiRegion > config.region
Proxy      : 凭据.proxyUrl > config.proxyUrl > 无代理
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

## Docker

```bash
docker-compose up
```

如果你走传统文件模式，需要挂载 `config.json` 和 `credentials.json`。
如果你走 Render / Git 持久化模式，则不需要这样挂载。

## 注意事项

1. 请妥善保管 RT 和 API Key
2. 多账号模式下，刷新后的 RT 会自动写回持久化后端
3. 代理环境异常时，可尝试把 `tlsBackend` 改成 `native-tls`
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
