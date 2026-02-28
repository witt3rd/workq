#!/usr/bin/env bash
set -euo pipefail

# Backup animus data plane and observability history.
#
# Usage:
#   ./scripts/backup.sh                  # full backup to ./backups/<timestamp>/
#   ./scripts/backup.sh /path/to/dir     # full backup to specific directory
#   ./scripts/backup.sh --db-only        # postgres only (skip observability)
#
# Requires: docker compose services running.

DB_ONLY=false
BACKUP_ROOT="./backups"

for arg in "$@"; do
  case "$arg" in
    --db-only) DB_ONLY=true ;;
    *) BACKUP_ROOT="$arg" ;;
  esac
done

TIMESTAMP=$(date +%Y%m%d-%H%M%S)
BACKUP_DIR="${BACKUP_ROOT}/${TIMESTAMP}"

mkdir -p "${BACKUP_DIR}"

echo "=== animus backup: ${TIMESTAMP} ==="

# --- Postgres (critical — work items, memories, queue state) ---
if docker compose ps postgres --status running -q 2>/dev/null | grep -q .; then
  echo "  pg_dump → animus.sql.gz"
  docker compose exec -T postgres \
    pg_dump -U animus -d animus_dev --clean --if-exists \
    | gzip > "${BACKUP_DIR}/animus.sql.gz"
  echo "  postgres: $(du -h "${BACKUP_DIR}/animus.sql.gz" | cut -f1)"
else
  echo "  ERROR: postgres not running"
  exit 1
fi

if [ "$DB_ONLY" = true ]; then
  echo "  (--db-only: skipping observability)"
else
  # --- Prometheus (metrics history) ---
  if docker compose ps prometheus --status running -q 2>/dev/null | grep -q .; then
    echo "  prometheus snapshot → prometheus-snapshot.tar.gz"
    # Trigger a snapshot via the admin API
    SNAP=$(curl -sf -X POST http://localhost:9090/api/v1/admin/tsdb/snapshot 2>/dev/null \
      | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['name'])" 2>/dev/null || echo "")
    if [ -n "$SNAP" ]; then
      docker compose exec -T prometheus \
        tar czf - -C /prometheus/data/snapshots "$SNAP" \
        > "${BACKUP_DIR}/prometheus-snapshot.tar.gz"
      # Clean up snapshot inside container
      docker compose exec -T prometheus rm -rf "/prometheus/data/snapshots/$SNAP"
      echo "  prometheus: $(du -h "${BACKUP_DIR}/prometheus-snapshot.tar.gz" | cut -f1)"
    else
      echo "  prometheus: snapshot failed (enable --web.enable-admin-api?)"
    fi
  else
    echo "  prometheus: not running, skipping"
  fi

  # --- Tempo (trace history — docker cp from volume) ---
  TEMPO_CONTAINER=$(docker compose ps tempo --status running -q 2>/dev/null || true)
  if [ -n "$TEMPO_CONTAINER" ]; then
    echo "  tempo traces → tempo-traces.tar.gz"
    docker cp "${TEMPO_CONTAINER}:/var/tempo/traces" - \
      | gzip > "${BACKUP_DIR}/tempo-traces.tar.gz"
    echo "  tempo: $(du -h "${BACKUP_DIR}/tempo-traces.tar.gz" | cut -f1)"
  else
    echo "  tempo: not running, skipping"
  fi

  # --- Loki (log history — docker cp from volume) ---
  LOKI_CONTAINER=$(docker compose ps loki --status running -q 2>/dev/null || true)
  if [ -n "$LOKI_CONTAINER" ]; then
    echo "  loki chunks → loki-chunks.tar.gz"
    docker cp "${LOKI_CONTAINER}:/loki/chunks" - \
      | gzip > "${BACKUP_DIR}/loki-chunks.tar.gz"
    echo "  loki: $(du -h "${BACKUP_DIR}/loki-chunks.tar.gz" | cut -f1)"
  else
    echo "  loki: not running, skipping"
  fi

  # --- Grafana dashboards ---
  if docker compose ps grafana --status running -q 2>/dev/null | grep -q .; then
    mkdir -p "${BACKUP_DIR}/grafana-dashboards"
    DASH_UIDS=$(curl -sf http://localhost:3000/api/search?type=dash-db 2>/dev/null \
      | python3 -c "import sys,json; [print(d['uid']) for d in json.load(sys.stdin)]" 2>/dev/null || true)
    if [ -n "${DASH_UIDS}" ]; then
      for uid in ${DASH_UIDS}; do
        curl -sf "http://localhost:3000/api/dashboards/uid/${uid}" \
          > "${BACKUP_DIR}/grafana-dashboards/${uid}.json" 2>/dev/null || true
      done
      DASH_COUNT=$(ls "${BACKUP_DIR}/grafana-dashboards/"*.json 2>/dev/null | wc -l | tr -d ' ')
      echo "  grafana: ${DASH_COUNT} dashboards"
    else
      echo "  grafana: no dashboards"
      rmdir "${BACKUP_DIR}/grafana-dashboards" 2>/dev/null || true
    fi
  else
    echo "  grafana: not running, skipping"
  fi
fi

# --- Manifest ---
COMPONENTS="\"postgres\": \"animus.sql.gz\""
if [ "$DB_ONLY" = false ]; then
  [ -f "${BACKUP_DIR}/prometheus-snapshot.tar.gz" ] && COMPONENTS="${COMPONENTS}, \"prometheus\": \"prometheus-snapshot.tar.gz\""
  [ -f "${BACKUP_DIR}/tempo-traces.tar.gz" ] && COMPONENTS="${COMPONENTS}, \"tempo\": \"tempo-traces.tar.gz\""
  [ -f "${BACKUP_DIR}/loki-chunks.tar.gz" ] && COMPONENTS="${COMPONENTS}, \"loki\": \"loki-chunks.tar.gz\""
fi

cat > "${BACKUP_DIR}/manifest.json" <<MANIFEST
{
  "timestamp": "${TIMESTAMP}",
  "version": "$(git describe --tags --always 2>/dev/null || echo 'unknown')",
  "retention": "30d",
  "components": { ${COMPONENTS} }
}
MANIFEST

TOTAL_SIZE=$(du -sh "${BACKUP_DIR}" | cut -f1)
echo "=== backup complete: ${BACKUP_DIR} (${TOTAL_SIZE}) ==="
