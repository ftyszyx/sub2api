# 本地测试

## 后端单元测试

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

对应入口见 [backend/Makefile](../backend/Makefile)。

## 后端集成测试

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

## 前端测试

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

## E2E 测试

当前仓库的 E2E 测试入口在 [backend/internal/integration](../backend/internal/integration) 下，使用 `e2e` build tag。

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

## 推荐的本地回归顺序

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
