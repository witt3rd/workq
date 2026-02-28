#!/usr/bin/env bash
set -euo pipefail

# Recover phase: log the error and exit 0 (requeue signal).
echo "recover: work_id=$ANIMUS_WORK_ID phase=$ANIMUS_PHASE" >&2
exit 0
