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

### 准备后端配置

先复制配置模板：

```bash
cp deploy/config.example.yaml backend/config.yaml
```

然后编辑 [backend/config.yaml](../backend/config.yaml)，至少确认这些配置：

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

### 启动后端
windows默认工作目录 是/app/data 就是当前盘的目录，需要指定 DATA_DIR 环境变量。

``````bash
cd backend
$env:DATA_DIR="."
go run ./cmd/server
```

如果是第一次运行且配置未完成，程序会进入 setup 流程。

### 启动前端

```bash
cd frontend
pnpm install
pnpm dev
```

前端开发服务器默认会代理到 `http://localhost:8080`，配置见 [frontend/vite.config.ts](../frontend/vite.config.ts)。

默认访问地址：

- 前端开发页：`http://localhost:3000`
- 后端服务：`http://localhost:8080`
