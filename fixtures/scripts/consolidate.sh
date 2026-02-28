#!/usr/bin/env bash
set -euo pipefail

# Consolidate phase: verify the transformation is correct.
original=$(jq -r '.content' orient-out.json)
result=$(jq -r '.result' engage-out.json)

# Reverse the result back — should match original
check=$(echo -n "$result" | rev)

if [ "$check" = "$original" ]; then
  jq -n --arg verdict "pass" --arg result "$result" --arg original "$original" \
    '{verdict: $verdict, result: $result, original: $original}' > consolidate-out.json
else
  jq -n --arg verdict "fail" --arg result "$result" --arg original "$original" --arg check "$check" \
    '{verdict: $verdict, result: $result, original: $original, check: $check}' > consolidate-out.json
  exit 1
fi
