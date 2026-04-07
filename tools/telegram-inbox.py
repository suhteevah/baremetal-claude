#!/usr/bin/env python3
"""telegram-inbox.py — fetch new messages from the Telegram bot.

Polls api.telegram.org/bot<TOKEN>/getUpdates with offset tracking so each
message is only seen once. Persists last-seen update_id to
.claude/telegram-offset.txt.

Reads TELEGRAM_BOT_TOKEN from .claude/.env (gitignored).

Usage:
  python tools/telegram-inbox.py             # human-readable, exits 0 even
                                             # if nothing new (loop-friendly)
  python tools/telegram-inbox.py --json      # machine output

Designed to be driven by /loop in Claude Code:
  /loop 1m check tools/telegram-inbox.py output and reply to any new
       messages via bash tools/notify-telegram.sh
"""

import argparse
import json
import os
import sys
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).parent.parent
ENV_FILE = REPO_ROOT / ".claude" / ".env"
OFFSET_FILE = REPO_ROOT / ".claude" / "telegram-offset.txt"


def load_env():
    if not ENV_FILE.exists():
        return
    for line in ENV_FILE.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        os.environ.setdefault(k.strip(), v.strip())


def read_offset() -> int:
    if OFFSET_FILE.exists():
        try:
            return int(OFFSET_FILE.read_text().strip())
        except ValueError:
            return 0
    return 0


def write_offset(off: int) -> None:
    OFFSET_FILE.parent.mkdir(parents=True, exist_ok=True)
    OFFSET_FILE.write_text(str(off))


def fetch_updates(token: str, offset: int) -> list:
    # offset = last_seen + 1 tells Telegram to drop everything <= last_seen
    url = f"https://api.telegram.org/bot{token}/getUpdates?offset={offset}&timeout=0"
    with urllib.request.urlopen(url, timeout=10) as r:
        data = json.loads(r.read().decode("utf-8"))
    if not data.get("ok"):
        raise RuntimeError(f"telegram api error: {data}")
    return data.get("result", [])


def format_message(upd: dict) -> dict | None:
    msg = upd.get("message") or upd.get("channel_post")
    if not msg or "text" not in msg:
        return None
    chat = msg.get("chat", {})
    sender = msg.get("from", {})
    return {
        "update_id": upd["update_id"],
        "message_id": msg.get("message_id"),
        "chat_id": chat.get("id"),
        "from": f"{sender.get('first_name', '?')} {sender.get('last_name', '')}".strip(),
        "date": msg.get("date"),
        "text": msg["text"],
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    load_env()
    token = os.environ.get("TELEGRAM_BOT_TOKEN")
    if not token:
        print("ERROR: TELEGRAM_BOT_TOKEN not set in .claude/.env", file=sys.stderr)
        sys.exit(1)

    last_seen = read_offset()
    updates = fetch_updates(token, last_seen + 1 if last_seen else 0)

    messages = []
    max_id = last_seen
    for upd in updates:
        max_id = max(max_id, upd["update_id"])
        m = format_message(upd)
        if m:
            messages.append(m)

    if max_id > last_seen:
        write_offset(max_id)

    if args.json:
        print(json.dumps({"new": len(messages), "messages": messages}, indent=2))
    elif messages:
        print(f"=== {len(messages)} new telegram message(s) ===")
        for m in messages:
            print(f"[{m['from']}] {m['text']}")
        print("=== end ===")
    else:
        print("(no new telegram messages)")


if __name__ == "__main__":
    main()
