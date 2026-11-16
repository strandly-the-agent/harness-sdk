#!/usr/bin/env python3
# Minimal stdio MCP server: tools echo, mutate (adds tool "extra" and notifies list_changed).
import json, os, sys
LOG = os.environ.get("R12_MCP_LOG", "/tmp/review12_correctness/mcp/log.txt")
NAME = os.environ.get("R12_MCP_NAME", "srv")
def log(msg):
    with open(LOG, "a") as f: f.write(f"{NAME} {os.getpid()} {msg}\n")
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n"); sys.stdout.flush()
tools = [
    {"name": "echo", "description": "echo text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
    {"name": "mutate", "description": "add a tool", "inputSchema": {"type": "object", "properties": {}}},
]
log("start")
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    msg = json.loads(line)
    method = msg.get("method"); mid = msg.get("id")
    if method == "initialize":
        log("initialize")
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": msg["params"].get("protocolVersion", "2025-03-26"), "capabilities": {"tools": {"listChanged": True}}, "serverInfo": {"name": NAME, "version": "0.0.1"}}})
    elif method == "tools/list":
        log("tools/list")
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": tools}})
    elif method == "tools/call":
        name = msg["params"]["name"]; log(f"tools/call {name}")
        if name == "mutate":
            if not any(t["name"] == "extra" for t in tools):
                tools.append({"name": "extra", "description": "added later", "inputSchema": {"type": "object", "properties": {}}})
            send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "mutated"}]}})
        else:
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "echo:" + str(msg["params"].get("arguments", {}).get("text"))}]}})
    elif method == "ping":
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "method not found"}})
log("exit")
