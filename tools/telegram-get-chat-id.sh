#!/usr/bin/env bash
# telegram-get-chat-id.sh — print the chat_id of the most recent message
# sent to your bot. Run this once after creating the bot via @BotFather:
#
#   1. Open Telegram, find your new bot, send it ANY message (e.g. "hi")
#   2. TELEGRAM_BOT_TOKEN=<token> bash tools/telegram-get-chat-id.sh
#   3. Copy the printed chat_id into .claude/.env
#
# Reads token from .claude/.env or env var.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$REPO_ROOT/.claude/.env"
if [[ -f "$ENV_FILE" ]]; then
    # shellcheck disable=SC1090
    set -a; source "$ENV_FILE"; set +a
fi

: "${TELEGRAM_BOT_TOKEN:?TELEGRAM_BOT_TOKEN not set}"

RESPONSE="$(curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getUpdates")"

# Pull all distinct chat ids + first names without requiring jq.
echo "$RESPONSE" | python -c '
import json, sys
data = json.load(sys.stdin)
if not data.get("ok"):
    print("API error:", data, file=sys.stderr); sys.exit(1)
seen = set()
for upd in data.get("result", []):
    msg = upd.get("message") or upd.get("channel_post") or {}
    chat = msg.get("chat", {})
    cid = chat.get("id")
    if cid is None or cid in seen:
        continue
    seen.add(cid)
    name = chat.get("first_name") or chat.get("title") or "?"
    typ = chat.get("type", "?")
    print(f"chat_id={cid}\ttype={typ}\tname={name}")
if not seen:
    print("No messages found. Send your bot any message first, then re-run.", file=sys.stderr)
    sys.exit(2)
'
