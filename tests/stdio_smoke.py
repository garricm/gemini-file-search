"""Test the MCP process without API access. Build the binary first."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

binary = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parents[1] / "target/debug/gemini-file-search-mcp"
with tempfile.TemporaryDirectory() as root:
    environment = {**os.environ, "GEMINI_FILE_SEARCH_API_KEY": "offline-placeholder", "GEMINI_UPLOAD_ROOT": root}
    messages = [
        {"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{"protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"offline-test", "version":"1"}}},
        {"jsonrpc":"2.0", "method":"notifications/initialized"},
        {"jsonrpc":"2.0", "id":2, "method":"tools/list", "params":{}},
        {"jsonrpc":"2.0", "id":3, "method":"tools/call", "params":{"name":"gemini_get_operation", "arguments":{"name":"https://evil.example/steal"}}},
    ]
    result = subprocess.run([str(binary)], input="\n".join(map(json.dumps,messages))+"\n", text=True, capture_output=True, env=environment, timeout=15)
    assert result.returncode == 0, result.stderr
    responses = {item["id"]:item for line in result.stdout.splitlines() if "id" in (item:=json.loads(line))}
    assert "serverInfo" in responses[1]["result"]
    tools = responses[2]["result"]["tools"]
    assert {tool["name"] for tool in tools} == {"gemini_create_store","gemini_list_stores","gemini_upload_file","gemini_get_operation"}
    annotations = {tool["name"]:tool["annotations"] for tool in tools}
    assert annotations["gemini_list_stores"]["readOnlyHint"] is True
    assert annotations["gemini_upload_file"]["idempotentHint"] is False
    assert responses[3]["result"]["isError"] is True
    assert "offline-placeholder" not in result.stdout+result.stderr
print("The MCP checks passed. The test did not call Google.")
