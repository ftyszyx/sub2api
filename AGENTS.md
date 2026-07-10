# AGENTS.md

## 测试规则
在自测完后，如果有启动服务，需要关闭。

## 代码规范
需要考虑扩展性，模块化,结构清晰，和易维护性

## GitHub Actions
只保留 tag release 使用的 `.github/workflows/release.yml`。同步 upstream 时，不要恢复其它 GitHub Actions workflow。
