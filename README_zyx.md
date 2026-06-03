# Sub2API 本地构建与测试指南

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

## 2. 本地开发启动方式

Docker Compose 开发环境和源码方式启动说明已移至 [doc_zyx/Development.md](doc_zyx/Development.md)。

## 5. 本地测试

本地测试说明已移至 [doc_zyx/Test.md](doc_zyx/Test.md)。
