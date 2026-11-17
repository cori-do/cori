#!/usr/bin/env bash
# Validates plugin.json and mcp.json against the Agent Plugins 1.0.0 schemas (fetched each time; needs Node.js).
set -euo pipefail
cd "$(dirname "$0")/.."
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
for s in plugin mcp; do
  curl -sSf -o "$TMP/$s.schema.json" "https://agent-plugins.org/schemas/1.0.0/$s.schema.json"
  npx -y -p ajv-cli@5 -p ajv-formats@3 ajv validate --spec=draft2020 --strict=false -c ajv-formats -s "$TMP/$s.schema.json" -d "$s.json"
done
