# Docker Compose 开发环境

如果你只是想最快把项目跑起来，推荐用开发版 compose。

## 准备 `.env`

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

## 构建并启动

当前仓库根目录 [Dockerfile](../Dockerfile) 里的 Go 基础镜像仍是 `golang:1.26.1-alpine`，而 `backend/go.mod` 已要求 `1.26.2`。因此建议显式覆盖构建参数：

```bash
cd deploy
docker compose -f docker-compose.dev.yml build  --no-cache

docker compose -f docker-compose.dev.yml up -d
```

## 验证服务是否启动成功

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

对应配置见 [deploy/docker-compose.dev.yml](../deploy/docker-compose.dev.yml)。
