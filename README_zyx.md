# Sub2API 本地构建与测试指南

这份文档面向本地开发、联调和回归测试，重点说明：

- 如何在本地启动项目
- 如何分别构建前端、后端和完整二进制
- 如何运行 unit、integration、frontend、e2e 测试
- 当前仓库里几个容易踩到的版本和脚本问题

## 1. 环境要求

建议先准备以下环境：

- Go 1.26.2
- Node.js 20+
- pnpm 9+
- Docker Desktop

说明：

- 后端版本以 [backend/go.mod](backend/go.mod) 为准，当前是 `go 1.26.2`
- 仓库里部分文件仍可能写着旧版本号，例如 CI 或 Dockerfile 中出现 `1.26.1` / `1.25.7`
- 本地构建时请优先以 `backend/go.mod` 为准

## 2. 最快启动方式：Docker Compose 开发环境

如果你只是想最快把项目跑起来，推荐用开发版 compose。

### 2.1 准备 `.env`

在项目根目录执行：

```bash
cd deploy
cp .env.example .env
```

至少建议修改这些值：

```env
POSTGRES_PASSWORD=sub2api
POSTGRES_USER=sub2api
POSTGRES_DB=sub2api

ADMIN_EMAIL=admin@sub2api.local
ADMIN_PASSWORD=admin123

JWT_SECRET=dev-jwt-secret
TOTP_ENCRYPTION_KEY=dev-totp-secret
SERVER_PORT=8080
```

### 2.2 构建并启动

当前仓库根目录 [Dockerfile](Dockerfile) 里的 Go 基础镜像仍是 `golang:1.26.1-alpine`，而 `backend/go.mod` 已要求 `1.26.2`。因此建议显式覆盖构建参数：

```bash
cd deploy
docker compose -f docker-compose.dev.yml build  --no-cache

docker compose -f docker-compose.dev.yml up -d
```


### 2.3 验证服务是否启动成功

```bash
docker compose -f docker-compose.dev.yml ps
docker compose -f docker-compose.dev.yml logs -f sub2api
curl http://127.0.0.1:8080/health
```

启动成功后可访问：

- 后端接口：`http://127.0.0.1:8080`
- 健康检查：`http://127.0.0.1:8080/health`

这份 compose 会自动启动：

- `sub2api`
- `postgres`
- `redis`

对应配置见 [deploy/docker-compose.dev.yml](deploy/docker-compose.dev.yml)。

## 3. 源码方式启动

如果你希望本地边改代码边调试，推荐数据库和 Redis 用 Docker，前后端用源码启动。

### 3.1 仅启动 PostgreSQL 和 Redis

```bash
cd deploy
docker compose -f docker-compose.dev.yml up -d postgres redis
```

### 3.2 准备后端配置

先复制配置模板：

```bash
cp deploy/config.example.yaml backend/config.yaml
```

然后编辑 [backend/config.yaml](backend/config.yaml)，至少确认这些配置：

- `server.host`
- `server.port`
- `database.host`
- `database.port`
- `database.user`
- `database.password`
- `database.dbname`
- `redis.host`
- `redis.port`
- `redis.password`
- `jwt.secret`

如果你只是本地开发，数据库和 Redis 可以直接指向：

- PostgreSQL: `127.0.0.1:5432`
- Redis: `127.0.0.1:6379`

如果前端页面不是通过本项目自带代理访问后端，而是浏览器直接从另一个源请求接口，例如页面在 `http://127.0.0.1:4321`、接口在 `https://sub2api.1postpro.com`，那就需要在后端配置里显式放行该来源。否则浏览器预检 `OPTIONS` 会失败，并出现类似下面的报错：

```text
No 'Access-Control-Allow-Origin' header is present on the requested resource
```

可以在实际运行使用的 `config.yaml` 中加入：

```yaml
cors:
  allowed_origins:
    - "http://127.0.0.1:4321"
    - "http://localhost:4321"
  allow_credentials: true
```

补充说明：

- 源码方式启动时，通常修改 `backend/config.yaml`
- Docker / 1Panel 部署时，通常修改 `deploy/data/config.yaml`
- 如果临时想对所有来源开放，可以写成 `allowed_origins: ["*"]`，但这时 `allow_credentials` 必须设为 `false`
- 修改后需要重启后端服务

### 3.3 启动后端

```bash
cd backend
go run ./cmd/server
```

如果是第一次运行且配置未完成，程序会进入 setup 流程。

### 3.4 启动前端

```bash
cd frontend
pnpm install
pnpm dev
```

前端开发服务器默认会代理到 `http://localhost:8080`，配置见 [frontend/vite.config.ts](frontend/vite.config.ts)。

默认访问地址：

- 前端开发页：`http://localhost:3000`
- 后端服务：`http://localhost:8080`

## 4. 本地构建

### 4.1 构建前端

```bash
cd frontend
pnpm install
pnpm build
```

前端产物默认输出到：

```text
backend/internal/web/dist
```

### 4.2 构建后端

```bash
cd backend
go build -ldflags="-s -w -X main.Version=$(tr -d '\r\n' < ./cmd/server/VERSION)" -trimpath -o bin/server ./cmd/server
```

Windows PowerShell 下如果你不想处理 `$(...)`，也可以直接：

```powershell
cd backend
go build -trimpath -o bin/server ./cmd/server
```

### 4.3 构建带前端嵌入的完整二进制

如果你想得到和部署更接近的完整二进制，先构建前端，再使用 `embed` 标签构建后端：

```bash
cd frontend
pnpm install
pnpm build

cd ../backend
go build -tags embed -o sub2api ./cmd/server
```

## 5. 本地测试

## 5.1 后端单元测试

和 CI 一致的命令是：

```bash
cd backend
go test -tags=unit ./...
```

也可以使用 Makefile：

```bash
cd backend
make test-unit
```

对应入口见 [backend/Makefile](backend/Makefile)。

## 5.2 后端集成测试

后端集成测试依赖 `testcontainers`，因此本地必须保证 Docker Desktop 已启动。

```bash
cd backend
go test -tags=integration ./...
```

或：

```bash
cd backend
make test-integration
```

补充说明：

- 这类测试会自动拉起 PostgreSQL / Redis 容器
- 如果 Docker 不可用，部分测试会跳过，部分测试会失败

## 5.3 前端测试

```bash
cd frontend
pnpm install
pnpm test:run
pnpm typecheck
pnpm lint:check
```

如果要验证前端最终构建：

```bash
pnpm build
```

## 5.4 E2E 测试

当前仓库的 E2E 测试入口在 [backend/internal/integration](backend/internal/integration) 下，使用 `e2e` build tag。

推荐直接运行：

```bash
cd backend
go test -tags=e2e -v -timeout=300s ./internal/integration/...
```

或者：

```bash
cd backend
make test-e2e-local
```

E2E 运行前需要先把服务启动起来，默认访问：

- `BASE_URL=http://localhost:8080`

可用环境变量包括：

- `BASE_URL`
- `E2E_MOCK=true`
- `CLAUDE_API_KEY`
- `GEMINI_API_KEY`

说明：

- 如果设置 `E2E_MOCK=true`，会走 mock 模式
- 如果不启用 mock 且未提供真实 API Key，部分 E2E 测试会自动跳过

## 5.5 推荐的本地回归顺序

如果你改了后端接口，推荐按这个顺序跑：

```bash
cd backend
go test ./internal/handler ./internal/server/routes ./internal/service
go test -tags=unit ./...
go test -tags=integration ./...
```

如果你改了前端页面，推荐再跑：

```bash
cd frontend
pnpm test:run
pnpm typecheck
pnpm build
```

## 补充：CORS / 跨域说明

如果浏览器直接调用图片接口，例如 `/v1/images/generations` 或 `/v1/images/edits`，它们和其他 API 一样都会走全局 CORS 中间件；这不是图片接口单独配置的问题。

典型报错：

```text
Access to fetch at 'https://.../v1/images/generations' from origin 'http://127.0.0.1:4321' has been blocked by CORS policy
```

推荐配置：

```yaml
cors:
  allowed_origins:
    - "http://127.0.0.1:4321"
    - "http://localhost:4321"
  allow_credentials: true
```

说明：
- `allowed_origins` 留空时，后端会拒绝跨域预检请求
- `allow_credentials: true` 不能与 `allowed_origins: ["*"]` 同时使用
- 如果前端 dev server 已经把请求代理到同源后端，一般不需要额外处理 CORS
- 配置改完后，记得重启服务再验证

## 6. 与 CI 对齐的命令

当前 CI 主要跑这几项：

```bash
cd backend
make test-unit
make test-integration
```

对应 workflow 在 [.github/workflows/backend-ci.yml](.github/workflows/backend-ci.yml)。

如果你想本地尽量贴近 CI，可以按下面顺序执行：

```bash
cd backend
go test -tags=unit ./...
go test -tags=integration ./...

cd ../frontend
pnpm install
pnpm test:run
pnpm typecheck
pnpm build
```

## 7. 常见问题

### 7.1 Docker 构建时报 Go 版本不匹配

典型报错：

```text
go: go.mod requires go >= 1.26.2 (running go 1.26.1; GOTOOLCHAIN=local)
```

原因：

- [backend/go.mod](backend/go.mod) 要求 `go 1.26.2`
- 根目录 [Dockerfile](Dockerfile) 当前默认还是 `golang:1.26.1-alpine`

解决方式：

```bash
docker compose -f deploy/docker-compose.dev.yml build \
  --build-arg GOLANG_IMAGE=golang:1.26.2-alpine \
  --no-cache
```

### 7.2 单独使用 `backend/Dockerfile` 构建失败

[backend/Dockerfile](backend/Dockerfile) 当前仍是：

```dockerfile
FROM golang:1.25.7-alpine
```

如果你直接在 `backend/` 目录执行 `docker build`，同样可能因为 Go 版本过低而失败。建议：

- 临时改成 `golang:1.26.2-alpine`
- 或优先使用根目录多阶段 [Dockerfile](Dockerfile)

### 7.3 Windows 没有 `make`

Windows 上如果没有 `make`，直接使用原始命令即可：

```bash
cd backend
go test -tags=unit ./...
go test -tags=integration ./...
go test -tags=e2e -v -timeout=300s ./internal/integration/...
```

### 7.4 `make test-e2e` 可能无法使用

[backend/Makefile](backend/Makefile) 中的：

```makefile
test-e2e:
	./scripts/e2e-test.sh
```

但当前仓库里没有 `backend/scripts/e2e-test.sh`。因此本地请优先使用：

```bash
cd backend
make test-e2e-local
```

或者直接：

```bash
go test -tags=e2e -v -timeout=300s ./internal/integration/...
```

### 7.5 integration 测试卡住或失败

优先检查：

- Docker Desktop 是否已启动
- 是否能正常拉取 postgres / redis 镜像
- 本机代理是否影响 Docker 拉镜像

## 8. 一份最实用的本地开发命令清单

### 方式 A：只想快速跑起来

```bash
cd deploy
cp .env.example .env
docker compose -f docker-compose.dev.yml build \
  --build-arg GOLANG_IMAGE=golang:1.26.2-alpine \
  --no-cache
docker compose -f docker-compose.dev.yml up -d
```

### 方式 B：源码开发联调

```bash
cd deploy
docker compose -f docker-compose.dev.yml up -d postgres redis

cd ..
cp deploy/config.example.yaml backend/config.yaml

cd backend
go run ./cmd/server

cd ../frontend
pnpm install
pnpm dev
```

### 方式 C：提交前本地回归

```bash
cd backend
go test -tags=unit ./...
go test -tags=integration ./...

cd ../frontend
pnpm test:run
pnpm typecheck
pnpm build
```
 
