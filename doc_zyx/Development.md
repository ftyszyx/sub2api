# 本地开发启动方式

## Docker Compose 开发环境

Docker Compose 开发环境说明已移至 [development_docker.md](development_docker.md)。

## 源码方式启动

如果你希望本地边改代码边调试，推荐数据库和 Redis 用 Docker，前后端用源码启动。

### 仅启动 PostgreSQL 和 Redis

```bash
cd deploy
docker compose -f docker-compose.dev.yml up -d postgres redis
```

### 启动后端

windows默认工作目录 是/app/data 就是当前盘的目录，需要指定 DATA_DIR 环境变量。

````bash
cd backend
go run ./cmd/server
```

如果是第一次运行且配置未完成，程序会进入 setup 流程。
再去修改/app/data/config.yaml
修改
```yaml
cors:
  allowed_origins:
    - "http://127.0.0.1:4321"
    - "http://localhost:4321"
  allow_credentials: true
```
- `server.port`

### 启动前端

## 增加
frontend/.env.local

## 设置端口

VITE_DEV_PROXY_TARGET=http://localhost:9080



```bash
cd frontend
pnpm install
pnpm dev
```

前端开发服务器默认会代理到 `http://localhost:8080`，配置见 [frontend/vite.config.ts](../frontend/vite.config.ts)。

默认访问地址：

- 前端开发页：`http://localhost:3000`
- 后端服务：`http://localhost:8080`
````
