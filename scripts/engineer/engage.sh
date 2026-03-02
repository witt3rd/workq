#!/usr/bin/env bash
set -euo pipefail

# Engage phase: placeholder until the built-in agentic loop exists.
#
# For now, this logs the work context and writes a minimal outcome.
# When the Rust engage loop (M5a) is built, this script will be
# replaced by the engine's built-in loop.

milestone=$(jq -r '.milestone' orient-out.json)
title=$(jq -r '.title' orient-out.json)
spec=$(jq -r '.spec' orient-out.json)

echo "engage: starting $milestone — $title" >&2
echo "engage: spec at $spec" >&2
echo "engage: (placeholder — no agentic loop yet)" >&2

jq -n \
  --arg milestone "$milestone" \
  --arg title "$title" \
  --arg status "acknowledged" \
  --arg note "Work item received by engineer faculty. Agentic engage loop not yet implemented (M5a). This placeholder confirms faculty routing works." \
  '{milestone: $milestone, title: $title, status: $status, note: $note}' \
  > engage-out.json
