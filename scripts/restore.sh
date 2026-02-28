#!/usr/bin/env bash
set -euo pipefail

# Restore animus from a backup.
#
# Usage:
#   ./scripts/restore.sh ./backups/20260228-031500/          # full restore
#   ./scripts/restore.sh ./backups/20260228-031500/ --db-only # postgres only
#
# WARNING: This replaces current data. Services must be running.

BACKUP_DIR="${1:?Usage: $0 <backup-dir> [--db-only]}"
DB_ONLY=false
[ "${2:-}" = "--db-only" ] && DB_ONLY=true

if [ ! -f "${BACKUP_DIR}/animus.sql.gz" ]; then
  echo "error: ${BACKUP_DIR}/animus.sql.gz not found"
  exit 1
fi

if [ -f "${BACKUP_DIR}/manifest.json" ]; then
  echo "=== restoring from backup ==="
  cat "${BACKUP_DIR}/manifest.json"
  echo ""
fi

echo "WARNING: This will replace data in the running services."
read -rp "Continue? [y/N] " confirm
if [[ ! "${confirm}" =~ ^[Yy]$ ]]; then
  echo "aborted"
  exit 0
fi

# --- Postgres ---
echo "  restoring postgres ..."
gunzip -c "${BACKUP_DIR}/animus.sql.gz" \
  | docker compose exec -T postgres psql -U animus -d animus_dev --quiet
echo "  postgres: done"

if [ "$DB_ONLY" = true ]; then
  echo "=== restore complete (db-only) ==="
  exit 0
fi

# --- Tempo ---
if [ -f "${BACKUP_DIR}/tempo-traces.tar.gz" ]; then
  echo "  restoring tempo traces ..."
  docker compose exec -T tempo rm -rf /var/tempo/traces
  docker compose exec -T tempo mkdir -p /var/tempo/traces
  cat "${BACKUP_DIR}/tempo-traces.tar.gz" \
    | docker compose exec -T tempo tar xzf - -C /var/tempo
  echo "  tempo: done (restart tempo to pick up restored data)"
fi

# --- Loki ---
if [ -f "${BACKUP_DIR}/loki-chunks.tar.gz" ]; then
  echo "  restoring loki chunks ..."
  docker compose exec -T loki rm -rf /loki/chunks
  docker compose exec -T loki mkdir -p /loki/chunks
  cat "${BACKUP_DIR}/loki-chunks.tar.gz" \
    | docker compose exec -T loki tar xzf - -C /loki
  echo "  loki: done (restart loki to pick up restored data)"
fi

# --- Prometheus ---
if [ -f "${BACKUP_DIR}/prometheus-snapshot.tar.gz" ]; then
  echo "  prometheus: snapshot restore requires manual import"
  echo "    see: https://prometheus.io/docs/prometheus/latest/storage/#backups"
  echo "    archive at: ${BACKUP_DIR}/prometheus-snapshot.tar.gz"
fi

echo "=== restore complete ==="
