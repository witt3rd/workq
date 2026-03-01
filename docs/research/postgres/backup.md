---
status: research
date: 2026-02-27
informed: docs/db.md, docker-compose.yml
---

# postgres backups when running as linux service systemd

You typically don’t back up “via the PostgreSQL systemd service” itself; instead you run separate systemd units (plus timers) that call `pg_dump`/`pg_dumpall` or `pg_basebackup` as the `postgres` user, with retention and monitoring built in.[^1_1][^1_2][^1_3]

## Overall approach

- Keep the regular PostgreSQL systemd unit just for running the server.
- Add one or more systemd **service** units whose `ExecStart` runs your backup script or `pg_dump`/`pg_basebackup`.
- Add **timer** units to schedule those services (daily, hourly, etc.).[^1_2][^1_4][^1_5]
- Run backup units as `User=postgres` (or another dedicated account with appropriate DB permissions), and write backups to a directory owned by that user.[^1_3][^1_2]


## Example: logical dump backups with systemd

1. Create a backup script, for example `/usr/local/sbin/pg_backup.sh`:
```bash
#!/usr/bin/env bash
set -euo pipefail

BACKUP_DIR=/srv/backups/postgresql/daily
RETENTION_DAYS=10
DB_NAME="$1"

mkdir -p "$BACKUP_DIR"

TS=$(date +%Y%m%d-%H%M%S)
FILE="${BACKUP_DIR}/${DB_NAME}-${TS}.sql.gz"

pg_dump --format=plain --dbname="$DB_NAME" | gzip > "$FILE"

find "$BACKUP_DIR" -type f -mtime +$RETENTION_DAYS -delete
```

- Make it executable: `chmod +x /usr/local/sbin/pg_backup.sh`.
- This is similar to patterns where a systemd service runs a dump command into a timestamped file and then deletes old backups.[^1_6][^1_2]

2. Create a systemd service unit, e.g. `/etc/systemd/system/pg-backup@.service`:
```ini
[Unit]
Description=PostgreSQL backup for %i
Requires=postgresql.service
After=postgresql.service

[Service]
Type=oneshot
User=postgres
Group=postgres
ExecStart=/usr/local/sbin/pg_backup.sh %i
```

- Templated `%i` lets you run `pg-backup@mydb.service` for each database.[^1_2]
- `Requires=postgresql.service` ensures the DB is up when the backup runs.[^1_2]

3. Create the timer unit `/etc/systemd/system/pg-backup@.timer`:
```ini
[Unit]
Description=Run PostgreSQL backup for %i

[Timer]
OnCalendar=daily
Persistent=true
Unit=pg-backup@%i.service

[Install]
WantedBy=timers.target
```

- `OnCalendar=daily` can be changed to `*-*-* 02:00:00` or similar.[^1_4][^1_5]
- `Persistent=true` runs missed timers after boot.[^1_4]

4. Enable and start timers:
```bash
systemctl daemon-reload
systemctl enable --now pg-backup@mydb.timer
systemctl list-timers 'pg-backup*'
```

This pattern avoids putting backup logic into the main PostgreSQL unit and gives first-class observability (`systemctl status`, `journalctl -u pg-backup@mydb.service`).[^1_5][^1_2]

## Example: base backup + WAL archiving (for PITR)

For point-in-time recovery you combine:

- Periodic base backups via `pg_basebackup`.
- Continuous archiving of WAL (e.g., `archive_mode=on`, `archive_command` to copy WAL to backup storage).[^1_7][^1_1][^1_3]

A oneshot systemd service might look like:

```ini
[Unit]
Description=PostgreSQL base backup
Requires=postgresql.service
After=postgresql.service

[Service]
Type=oneshot
User=postgres
Group=postgres
Environment=PGUSER=replica
Environment=PGPASSWORD=...
ExecStart=/usr/bin/pg_basebackup -D /srv/backups/postgresql/base/%Y%m%d-%H%M%S -F tar -z -X stream -P
```

- Backups plus WAL archiving enable recovery to an arbitrary point in time, not just to the backup instant.[^1_1][^1_3][^1_7]
- Schedule this via a timer (e.g., weekly) and store WAL archives in a durable location (object storage, NAS, etc.).[^1_3][^1_7]


## Operational considerations

- **Isolation:** Keep backup units separate from the main `postgresql.service`; don’t stop the DB for logical dumps, and only stop it for filesystem-level backups when you really must.[^1_8][^1_1]
- **Security:** Restrict backup directory permissions, and encrypt backups at rest/in transit where needed.[^1_3]
- **Retention:** Implement retention in your script (e.g., `find ... -mtime +N -delete`) or an external backup system.[^1_6][^1_2][^1_3]
- **Verification:** Periodically restore backups to a scratch instance to verify that your systemd jobs are producing usable backups.[^1_1][^1_3]

If you share your current PostgreSQL systemd unit and what kind of backups you want (logical vs PITR), I can tailor concrete unit/timer files and scripts for your exact setup.
<span style="display:none">[^1_10][^1_11][^1_12][^1_13][^1_14][^1_15][^1_9]</span>

<div align="center">⁂</div>

[^1_1]: https://www.crunchydata.com/blog/introduction-to-postgres-backups

[^1_2]: https://dailystuff.nl/blog/2023/use-systemd-timers-for-postgresql-dumps.html

[^1_3]: https://dev.to/dmetrovich/postgresql-backup-and-restore-complete-guide-to-backing-up-and-restoring-databases-690

[^1_4]: https://oneuptime.com/blog/post/2026-02-20-linux-systemd-services/view

[^1_5]: https://yieldcode.blog/post/working-with-systemd-timers/

[^1_6]: https://oneuptime.com/blog/post/2026-01-25-postgresql-automated-backups-pg-dump/view

[^1_7]: https://docs.microfocus.com/doc/amx/25.1/selfmanagedpgbackuprestore

[^1_8]: https://learnomate.org/postgresql-backup-and-restoration-guide/

[^1_9]: https://community.commvault.com/share-best-practices-3/postgres-fs-based-best-practice-for-log-backups-8946

[^1_10]: https://www.zimmi.cz/posts/2018/postgresql-backup-and-recovery-orchestration-systemd-automation/

[^1_11]: https://fale.io/blog/2024/05/31/perform-backups-with-systemd

[^1_12]: https://gist.github.com/speratus/b2b65f647629ec088e4ae244a42edcf1

[^1_13]: https://stackoverflow.com/questions/52168081/systemd-unit-for-pgagent

[^1_14]: http://ljwrites.blog/posts/backup-systemd-timer/

[^1_15]: https://docs.redhat.com/en/documentation/red_hat_enterprise_linux/9/html/configuring_and_using_database_servers/using-postgresql_configuring-and-using-database-servers

