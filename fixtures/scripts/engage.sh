#!/usr/bin/env bash
set -euo pipefail

# Engage phase: apply the transformation (reverse the content).
content=$(jq -r '.content' orient-out.json)
reversed=$(echo -n "$content" | rev)

jq -n --arg result "$reversed" '{result: $result}' > engage-out.json
