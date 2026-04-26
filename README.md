# CodexManager

这是一个 macOS 菜单栏 Tauri App，用官方 `codex app-server` 管理本机 Codex 配置、会话和 Telegram 入口。

## 当前范围

- Codex app-server 状态检查。
- Codex 供应商管理：新增、编辑、删除、激活，激活时写入 `~/.codex/config.toml` 和 `~/.codex/auth.json`。
- Codex 会话管理：通过 Codex app-server 的 `thread/list`、`thread/read`、`thread/archive` 获取、查看和归档会话。
- Telegram Bot 配置：维护 `.runtime.env`，通过 launchd 重启本机 bot。
- Telegram Bot 运行时：Rust 二进制 `telegram-codex-bot`，普通文本通过 app-server 发给 Codex，独立新对话不绑定目录，项目新对话使用项目 cwd。

不包含 MCP/Skills 同步。

## 关键文件

- 前端入口：`src/main.js`
- 前端数据适配：`src/status.js`
- 样式：`src/styles.css`
- Tauri command：`src-tauri/src/lib.rs`
- app-server 客户端：`src-tauri/src/app_server.rs`
- 供应商管理：`src-tauri/src/codex_provider.rs`
- 会话管理：`src-tauri/src/app_server.rs`
- TG Bot 设置：`src-tauri/src/bot_settings.rs`
- TG Bot 二进制入口：`src-tauri/src/bin/telegram-codex-bot.rs`
- launchd 启动脚本：`scripts/run-bot.sh`

## 本地运行

```sh
npm install
npm run tauri -- dev
```

默认通过 PATH 中的 `codex` 命令启动 `codex app-server`。

开发模式会临时启动 Vite 调试端口；打包后的 App 使用内置 `dist` 静态文件，不需要也不会依赖前端端口服务。

## 打包

```sh
npm run tauri -- build --debug
```

调试版 App 生成位置：

```text
src-tauri/target/debug/bundle/macos/CodexManager.app
```

## Telegram Bot

复制 `.runtime.env.example` 为 `.runtime.env`，至少配置：

```sh
TELEGRAM_BOT_TOKEN="BotFather token"
TELEGRAM_ALLOWED_USER_ID="你的 Telegram user id"
CODEX_PATH="codex"
CODEX_BOT_DROP_PENDING_UPDATES="true"
```

`CODEX_PATH` 可选；不配置时使用 PATH 中的 `codex`。

`CODEX_BOT_DROP_PENDING_UPDATES` 默认开启。Bot 每次启动时会丢弃 Telegram 离线期间积压的 update，避免服务恢复后突然执行旧消息。

启动脚本会读取 `.runtime.env` 并执行：

```text
src-tauri/target/debug/telegram-codex-bot
```

当前 launchd label：

```text
com.local.telegram-codex-bot
```

## 验证

```sh
npm test
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri -- build --debug
```
