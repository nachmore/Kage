#!/usr/bin/env python3
"""Inspect the kage-cli SQLite database to understand the conversation format."""
import sqlite3, json, os, sys

db_path = os.path.join(os.environ.get("LOCALAPPDATA", ""), "kage-cli", "data.sqlite3")
if not os.path.exists(db_path):
    print(f"Database not found: {db_path}")
    sys.exit(1)

db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
cur = db.cursor()

# List tables
cur.execute("SELECT name FROM sqlite_master WHERE type='table'")
print("Tables:", [r[0] for r in cur.fetchall()])

# Count conversations
cur.execute("SELECT COUNT(*) FROM conversations_v2")
print(f"Conversations: {cur.fetchone()[0]}")

# Sample recent conversations
cur.execute("SELECT key, conversation_id, value, created_at, updated_at FROM conversations_v2 ORDER BY updated_at DESC LIMIT 3")
for row in cur.fetchall():
    key, conv_id, value_json, created, updated = row
    data = json.loads(value_json)
    print(f"\n--- Conversation: {conv_id[:20]}...")
    print(f"  Key (workspace): {key}")
    print(f"  Created: {created}, Updated: {updated}")
    print(f"  Top-level keys: {list(data.keys())}")
    if "title" in data:
        print(f"  Title: {data['title']}")
    if "messages" in data:
        msgs = data["messages"]
        print(f"  Messages: {len(msgs)}")
        for m in msgs[:5]:
            role = m.get("role", "?")
            content = m.get("content", "")
            if isinstance(content, list):
                preview = str(content[0])[:80] if content else "[]"
            elif isinstance(content, str):
                preview = content[:80]
            else:
                preview = str(content)[:80]
            print(f"    [{role}] {preview}")
    if "history" in data:
        hist = data["history"]
        print(f"  History entries: {len(hist)}")
        for h in hist[:3]:
            if isinstance(h, dict):
                print(f"    keys: {list(h.keys())}")
                print(f"    sample: {json.dumps(h, default=str)[:200]}")
            elif isinstance(h, list):
                print(f"    [list] len={len(h)}")
                if h:
                    print(f"    first: {json.dumps(h[0], default=str)[:200]}")
            else:
                print(f"    type={type(h).__name__}: {str(h)[:100]}")
    if "transcript" in data:
        trans = data["transcript"]
        print(f"  Transcript entries: {len(trans)}")
        for t in trans[:8]:
            if isinstance(t, dict):
                role = t.get("role", "?")
                content = t.get("content", "")
                if isinstance(content, list):
                    types = [c.get("type", "?") for c in content[:5]]
                    texts = [c.get("text", "")[:80] for c in content if c.get("type") == "text" and c.get("text")]
                    print(f"    [{role}] types={types}")
                    if texts:
                        print(f"           text: {texts[0]}")
                elif isinstance(content, str):
                    print(f"    [{role}] {content[:100]}")

db.close()
