#!/usr/bin/env bash
set -euo pipefail

# Orient phase: extract work item context for the engage phase.
milestone=$(jq -r '.params.milestone // "unknown"' work.json)
title=$(jq -r '.params.title // "unknown"' work.json)
description=$(jq -r '.params.description // ""' work.json)
spec=$(jq -r '.params.spec // ""' work.json)

jq -n \
  --arg milestone "$milestone" \
  --arg title "$title" \
  --arg description "$description" \
  --arg spec "$spec" \
  '{milestone: $milestone, title: $title, description: $description, spec: $spec}' \
  > orient-out.json

echo "orient: $milestone — $title" >&2
