#!/usr/bin/env bash
set -euo pipefail

# Orient phase: read work.json, extract content, write instruction.
content=$(jq -r '.params.content' work.json)

jq -n --arg content "$content" --arg instruction "reverse" \
  '{instruction: $instruction, content: $content}' > orient-out.json
