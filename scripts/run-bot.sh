#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
ENV_FILE="$ROOT/.runtime.env"
BOT_BIN="$ROOT/src-tauri/target/debug/telegram-codex-bot"

if [ -f "$ENV_FILE" ]; then
  set -a
  . "$ENV_FILE"
  set +a
fi

export CODEX_PATH="${CODEX_PATH:-codex}"
export CODEX_BOT_DROP_PENDING_UPDATES="${CODEX_BOT_DROP_PENDING_UPDATES:-true}"

mkdir -p "$ROOT/data" "$ROOT/logs"

if [ ! -x "$BOT_BIN" ]; then
  echo "telegram bot binary 不存在或不可执行：$BOT_BIN" >&2
  exit 78
fi

if [ -z "${TELEGRAM_BOT_TOKEN:-}" ]; then
  echo "TELEGRAM_BOT_TOKEN 未配置，请写入 $ENV_FILE" >&2
  exit 78
fi

if [ -z "${TELEGRAM_ALLOWED_USER_ID:-}" ]; then
  echo "TELEGRAM_ALLOWED_USER_ID 未配置，请写入 $ENV_FILE" >&2
  exit 78
fi

cd "$ROOT"
exec "$BOT_BIN"
