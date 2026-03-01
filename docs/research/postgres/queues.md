---
status: research
date: 2026-02-27
informed: docs/db.md
---

# does postgres support queues

Postgres doesn’t have a first‑class “queue” type like RabbitMQ or SQS, but it’s commonly and effectively used to implement queues and job systems.

## How Postgres is used as a queue

- You store messages/jobs in a table (e.g., `jobs` with status, payload, timestamps).[^1_1][^1_7][^1_8]
- Workers dequeue using `SELECT ... FOR UPDATE SKIP LOCKED` so each job is locked by one worker and others skip it, which prevents double processing.[^1_7][^1_8]
- You can model visibility timeouts, retries, and status transitions (`pending` → `in_progress` → `done`/`failed`) with columns and simple SQL.[^1_8][^1_1]
- `LISTEN/NOTIFY` can be used to push-style notify workers that new work has arrived instead of pure polling.[^1_4]


## Extensions and ecosystem

- The `pgmq` extension (used by Supabase Queues) adds higher-level queue primitives (create queue, send, read with visibility timeout, archive, delayed messages) directly in Postgres.[^1_5][^1_6]
- Managed products like Supabase Queues build on this to provide basic, unlogged, and (soon) partitioned queues for higher throughput.[^1_5]


## When it’s a good idea

Using Postgres as a queue works well when:

- You already depend heavily on Postgres and want to avoid extra infra.[^1_1][^1_5]
- Throughput is moderate to high but not huge streaming-scale (teams report tens to hundreds of thousands of events/sec with careful tuning).[^1_2][^1_4]
- You benefit from transactional semantics between your domain data and the queue (enqueue in the same transaction as business updates).[^1_1][^1_5]

If you tell me your throughput and durability requirements (and whether you can add extensions), I can sketch a concrete schema and `enqueue`/`dequeue` SQL that fits your use case.
<span style="display:none">[^1_3][^1_9]</span>

<div align="center">⁂</div>

[^1_1]: https://dev.to/shrsv/turning-postgresql-into-a-robust-queue-for-go-applications-1hob

[^1_2]: https://www.rudderstack.com/blog/scaling-postgres-queue/

[^1_3]: https://www.reddit.com/r/PostgreSQL/comments/1edct6o/postgres_message_queue/

[^1_4]: https://news.ycombinator.com/item?id=39315833

[^1_5]: https://supabase.com/blog/supabase-queues

[^1_6]: https://userjot.com/blog/using-postgres-docker-pgmq-message-queue

[^1_7]: http://www.mgaillard.fr/2024/12/01/job-queue-postgresql.html

[^1_8]: https://aminediro.com/posts/pg_job_queue/

[^1_9]: https://elixirforum.com/t/sequin-elixir-and-postgres-for-a-feature-rich-message-queue/65015


---

# what about time-release queue items (i.e., future events) or timer-based?

You model time-release items by adding a “not before” timestamp to each job and including it in your dequeue query; Postgres handles this pattern well.

## Simple time-based scheduling

The usual schema pattern is to add a `run_at` (or `visible_at`) column and only let workers pick jobs whose time has come.  For example:[^2_1][^2_2]

```sql
CREATE TABLE jobs (
  id        bigserial primary key,
  status    text not null default 'pending',
  payload   jsonb,
  run_at    timestamptz not null default now(), -- schedule in future
  created_at timestamptz not null default now()
);

-- dequeue
WITH next AS (
  SELECT id
  FROM jobs
  WHERE status = 'pending'
    AND run_at <= now()
  ORDER BY run_at, id
  LIMIT 1
  FOR UPDATE SKIP LOCKED
)
UPDATE jobs
SET status = 'in_progress'
FROM next
WHERE jobs.id = next.id
RETURNING jobs.*;
```

This lets you enqueue with `run_at = now() + interval '10 minutes'` to create delayed jobs.[^2_2]

## Timer and retry semantics

You can unify “scheduled for the future” and “retry after backoff” by using the same timestamp column (often called `visible_at`) and advancing it on dequeue.[^2_3][^2_4][^2_1]

- New job: insert with `visible_at = desired_first_run_time`.[^2_1]
- Worker dequeue: `WHERE visible_at <= now()` plus `FOR UPDATE SKIP LOCKED`, then set `visible_at = now() + processing_time_estimate` (visibility timeout).[^2_4][^2_3][^2_1]
- On failure or retry: set `visible_at = now() + retry_backoff` and keep status `pending` or similar.[^2_1]

This pattern gives you both “do not run before X” and “re-run after Y if needed” with a single timer field.[^2_3][^2_4][^2_1]

## Using extensions / cron-like behavior

- `pgmq` exposes delayed messages and visibility timeouts directly in queue APIs; you pass a visibility-timeout (`vt`) and optional delay when reading/sending.[^2_4][^2_3]
- Postgres-based schedulers like PgQueuer or homegrown orchestrators combine a queue table with `run_at` and `LISTEN/NOTIFY` for real-time wakeups, effectively acting as a cron plus delayed-queue system.[^2_5][^2_6]

If you share your desired guarantees (e.g., “up to 1M delayed jobs”, “precision to seconds”, “max delay 30 days”), I can propose a concrete table definition, indexes, and dequeue SQL tuned for that regime.
<span style="display:none">[^2_10][^2_11][^2_12][^2_13][^2_14][^2_15][^2_7][^2_8][^2_9]</span>

<div align="center">⁂</div>

[^2_1]: https://aminediro.com/posts/pg_job_queue/

[^2_2]: https://brandur.org/postgres-queues

[^2_3]: https://api.pgxn.org/src/pgmq/pgmq-1.5.0/README.md

[^2_4]: https://github.com/pgmq/pgmq

[^2_5]: https://nango.dev/blog/migrating-from-temporal-to-a-postgres-based-task-orchestrator

[^2_6]: https://www.reddit.com/r/Python/comments/1kd6ci0/pgqueuer_postgresqlnative_job_schedule_queue/

[^2_7]: https://stackoverflow.com/questions/44209117/improving-query-time-for-a-job-queue-built-on-postgresql

[^2_8]: https://github.com/graphile/worker/issues/380

[^2_9]: https://www.inferable.ai/blog/posts/postgres-skip-locked

[^2_10]: https://github.com/tembo-io/pgmq/issues/70

[^2_11]: https://cetra3.github.io/blog/implementing-a-jobq/

[^2_12]: https://neon.com/guides/queue-system

[^2_13]: https://www.youtube.com/watch?v=m6-63kpttQk

[^2_14]: https://www.nisheetsinvhal.com/blog/there-are-no-no-brainer-decisions

[^2_15]: https://github.com/graphile/worker/issues/240


---

# sounds like pgmq has what we need

Yeah, pgmq basically bakes the patterns we’ve been talking about into an extension API, so you don’t have to hand-roll the queue tables and locking logic.[^3_1][^3_2]

## Why pgmq fits your use case

- Native to Postgres: It’s just an extension (`create extension pgmq;`), so you keep a single datastore and get transactional semantics with your existing tables.[^3_3][^3_1]
- Delayed / time-release messages: `pgmq.send(queue_name, msg, delay => seconds_or_timestamp)` lets you schedule future delivery; the delay parameter can be a relative number of seconds or an absolute timestamp.[^3_4][^3_1]
- Visibility timeouts and retries: `pgmq.read(queue_name, vt => seconds, qty => N)` reads messages and hides them from other consumers for `vt` seconds, which gives you SQS-style at-least-once delivery and retry behavior.[^3_1][^3_4]
- Exactly-once-ish APIs: There are both `read` (explicit ack via archive/delete) and `pop` (read-and-delete) styles, so you can pick between at-least-once with explicit ack or at-most-once semantics.[^3_5][^3_1]
- Archival / replay: Built-in `archive` functions move processed messages to an archive table instead of hard-deleting them, which is nice for debugging and audit.[^3_3][^3_1]


## How it might look in practice

- Create a queue once:

```sql
select pgmq.create('events_queue');
```

- Enqueue a timer-based event for 10 minutes from now:

```sql
select pgmq.send(
  queue_name => 'events_queue',
  msg        => '{"type":"do_something","id":123}'::jsonb,
  delay      => 600
);
```  [^3_1][^3_4]  
```

- Worker loop (pseudo-SQL):

```sql
select * from pgmq.read(
  queue_name => 'events_queue',
  vt         => 60,
  qty        => 10
);
-- process, then archive/delete by msg_id
```  [^3_1][^3_4]  

```


If you tell me your scale and failure semantics (e.g., “SQS-like at-least-once with up to 100k msgs/sec, mostly delayed jobs”), I can suggest concrete pgmq settings (vt, delays, queue partitioning, etc.) and an integration shape for your stack.
<span style="display:none">[^3_10][^3_11][^3_12][^3_13][^3_14][^3_15][^3_6][^3_7][^3_8][^3_9]</span>

<div align="center">⁂</div>

[^3_1]: https://supabase.com/docs/guides/queues/pgmq

[^3_2]: https://github.com/pgmq/pgmq

[^3_3]: https://supabase.com/features/queues

[^3_4]: https://userjot.com/blog/using-postgres-docker-pgmq-message-queue

[^3_5]: https://pgxn.org/dist/pgmq/1.1.1/docs/api/sql/functions.html

[^3_6]: https://aminediro.com/posts/pg_job_queue/

[^3_7]: https://github.com/oliverlambson/pgmq

[^3_8]: https://supabase.com/blog/supabase-queues

[^3_9]: https://alexn.org/blog/2022/10/21/modeling-queue-for-delayed-messages-via-rdbms/

[^3_10]: https://docs.rs/pgmq

[^3_11]: https://dev.to/leapcell/redis-delayed-queue-explained-once-and-for-all-51o8

[^3_12]: https://www.reddit.com/r/PostgreSQL/comments/1edct6o/postgres_message_queue/

[^3_13]: https://github.com/orgs/supabase/discussions/31201

[^3_14]: https://news.ycombinator.com/item?id=39315833

[^3_15]: https://news.ycombinator.com/item?id=37036256

