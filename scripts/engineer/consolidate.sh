#!/usr/bin/env bash
set -euo pipefail

# Consolidate phase: package the engage output as the work outcome.
cp engage-out.json consolidate-out.json

milestone=$(jq -r '.milestone' engage-out.json)
status=$(jq -r '.status' engage-out.json)
echo "consolidate: $milestone — $status" >&2
