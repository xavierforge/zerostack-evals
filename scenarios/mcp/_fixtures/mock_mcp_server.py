#!/usr/bin/env python3
"""Minimal MCP stdio server, stdlib only: exposes one tool, lookup_ticket(id)."""
import json
import sys

TICKETS = {
    "ZS-101": "ticket ZS-101: renderer flickers on resize - status: open",
}


def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def handle_initialize(req_id, params):
    protocol_version = (params or {}).get("protocolVersion", "2025-06-18")
    send({
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "protocolVersion": protocol_version,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "mock-mcp-server", "version": "0.1.0"},
        },
    })


def handle_tools_list(req_id):
    send({
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "tools": [
                {
                    "name": "lookup_ticket",
                    "description": "Look up a support ticket by its ID and return its status.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "description": "Ticket ID, e.g. ZS-101"}
                        },
                        "required": ["id"],
                    },
                }
            ]
        },
    })


def handle_tools_call(req_id, params):
    name = (params or {}).get("name")
    args = (params or {}).get("arguments") or {}
    if name != "lookup_ticket":
        send({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "content": [{"type": "text", "text": f"unknown tool: {name}"}],
                "isError": True,
            },
        })
        return
    ticket_id = args.get("id", "")
    text = TICKETS.get(ticket_id, f"no ticket found for id '{ticket_id}'")
    send({
        "jsonrpc": "2.0",
        "id": req_id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "isError": False,
        },
    })


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = msg.get("method")
        req_id = msg.get("id")
        params = msg.get("params")
        if method == "initialize":
            handle_initialize(req_id, params)
        elif method == "notifications/initialized":
            continue
        elif method == "tools/list":
            handle_tools_list(req_id)
        elif method == "tools/call":
            handle_tools_call(req_id, params)
        elif method == "ping":
            send({"jsonrpc": "2.0", "id": req_id, "result": {}})
        elif req_id is not None:
            send({
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {"code": -32601, "message": f"method not found: {method}"},
            })


if __name__ == "__main__":
    main()
