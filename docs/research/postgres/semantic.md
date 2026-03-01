---
status: research
date: 2026-02-27
informed: docs/db.md
---

# postgres semantic indexing search with metadata filtering

You can do this cleanly with pgvector plus normal Postgres WHERE clauses and (optionally) BM25/full‑text for hybrid search.[^1_1][^1_2]

## Core table and index

```sql
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE documents (
  id          bigserial PRIMARY KEY,
  title       text,
  body        text,
  -- arbitrary metadata
  tenant_id   uuid,
  doc_type    text,
  tags        text[],
  created_at  timestamptz NOT NULL DEFAULT now(),

  embedding   vector(1536)
);

-- ANN index for semantic search (cosine, choose to taste)
CREATE INDEX ON documents
USING hnsw (embedding vector_cosine_ops);

-- Optional: partial index for a hot tenant or type
CREATE INDEX ON documents
USING hnsw (embedding vector_cosine_ops)
WHERE doc_type = 'article';
```

HNSW or IVFFlat indexes give you scalable vector search; partial indexes or partitioning help if you filter on the same metadata a lot.[^1_3][^1_4][^1_2][^1_5]

## Basic semantic search with metadata filters

Assume you get an embedding for the user query on the app side:

```sql
-- :query_embedding :: vector
-- :tenant_id :: uuid
-- :doc_type :: text
-- :tags :: text[]
-- :limit :: int

SELECT id, title, body,
       embedding <-> :query_embedding AS distance
FROM documents
WHERE tenant_id = :tenant_id                 -- exact metadata
  AND doc_type = :doc_type                   -- exact metadata
  AND tags && :tags                          -- array overlap, if needed
ORDER BY embedding <-> :query_embedding      -- semantic ranking
LIMIT :limit;
```

Metadata filtering is just normal SQL in the WHERE clause; pgvector only changes how you rank rows via `<->` (distance) or `<#>` (inner product).[^1_2][^1_6][^1_1][^1_3]

Common patterns:

- Time‑bounded search: `AND created_at >= now() - interval '30 days'`.[^1_1]
- Multi‑tenant: `tenant_id` in the WHERE, with per‑tenant partial indexes if some tenants are large.[^1_6][^1_2]
- Soft filters: filter hard on some metadata (tenant, visibility) and leave others as post‑filtering or reranking in the app.


## Hybrid semantic + BM25 in Postgres

If you want “semantic + keyword”:

1. Add a tsvector column and GIN index:
```sql
ALTER TABLE documents
  ADD COLUMN search_lex tsvector
    GENERATED ALWAYS AS (
      to_tsvector('english', coalesce(title,'') || ' ' || coalesce(body,''))
    ) STORED;

CREATE INDEX documents_search_lex_gin
  ON documents
  USING gin (search_lex);
```

2. Query with both lexical and vector terms, ranking primarily by semantic distance:
```sql
-- :query_text, :query_embedding, :tenant_id, :limit

WITH base AS (
  SELECT id, title, body,
         ts_rank_cd(search_lex, plainto_tsquery('english', :query_text)) AS bm25,
         embedding <-> :query_embedding AS distance
  FROM documents
  WHERE tenant_id = :tenant_id
    AND search_lex @@ plainto_tsquery('english', :query_text)
)
SELECT *
FROM base
ORDER BY distance ASC, bm25 DESC
LIMIT :limit;
```

You can also do real BM25 via extensions like ParadeDB/pg_textsearch or VectorChord‑bm25, then fuse BM25 and vector scores or do RRF fusion in SQL or app code.[^1_7][^1_8][^1_9]

## Metadata‑aware indexing tips

- If you always filter by `tenant_id` or `doc_type`, consider:
    - Partial HNSW/IVFFlat indexes per value.[^1_2]
    - Table partitioning on that column, with an embedding index per partition.[^1_6][^1_2]
- If you filter on many different values (e.g., thousands of tenants), partial indexes per value can explode; prefer partitioning or a global index plus partition pruning.[^1_2][^1_6]
- Tune ANN parameters (`lists` for IVFFlat, `m` and `ef_search` for HNSW) for your recall/latency targets.[^1_4][^1_3]


## Example end‑to‑end query (filters + hybrid)

Putting it together:

```sql
WITH filtered AS (
  SELECT id, title, body, tags, created_at,
         ts_rank_cd(search_lex, plainto_tsquery('english', :q)) AS bm25,
         embedding <-> :query_embedding AS distance
  FROM documents
  WHERE tenant_id = :tenant_id
    AND doc_type = ANY(:doc_types)
    AND tags && :required_tags
    AND created_at >= :since
),
scored AS (
  SELECT *,
         -- simple score fusion: adjust weights to taste
         (0.7 * (1.0 / (1e-6 + distance)) + 0.3 * bm25) AS hybrid_score
  FROM filtered
)
SELECT *
FROM scored
ORDER BY hybrid_score DESC
LIMIT :k;
```

This gives you semantic ranking with hard metadata constraints and a tunable lexical/semantic blend, all staying inside Postgres.[^1_8][^1_9][^1_7][^1_1]

If you share your specific shape (RAG vs product search, expected row counts, typical filters), I can sketch concrete table/index definitions and a minimal Rust/Python access layer.
<span style="display:none">[^1_10][^1_11][^1_12][^1_13][^1_14][^1_15]</span>

<div align="center">⁂</div>

[^1_1]: https://www.red-gate.com/simple-talk/databases/postgresql/how-to-build-an-ai-powered-semantic-search-in-postgresql-with-pgvector/

[^1_2]: https://github.com/pgvector/pgvector

[^1_3]: https://severalnines.com/blog/vector-similarity-search-with-postgresqls-pgvector-a-deep-dive/

[^1_4]: https://www.datacamp.com/tutorial/pgvector-tutorial

[^1_5]: https://codesignal.com/learn/courses/indexing-optimization-and-scaling-pgvector/lessons/understanding-and-managing-indexes-in-pgvector

[^1_6]: https://www.yugabyte.com/key-concepts/using-postgresql-as-a-vector-database/

[^1_7]: https://docs.vectorchord.ai/vectorchord/use-case/hybrid-search.html

[^1_8]: https://www.paradedb.com/blog/hybrid-search-in-postgresql-the-missing-manual

[^1_9]: https://www.pedroalonso.net/blog/postgres-bm25-search/

[^1_10]: https://www.tigerdata.com/blog/implementing-filtered-semantic-search-using-pgvector-and-javascript-2

[^1_11]: https://aws.amazon.com/blogs/database/supercharging-vector-search-performance-and-relevance-with-pgvector-0-8-0-on-amazon-aurora-postgresql/

[^1_12]: https://github.com/vansh-khaneja/Semantic-Search-with-Filters-using-PostgreSQL-and-Pgvectors

[^1_13]: https://www.dataquest.io/blog/metadata-filtering-and-hybrid-search-for-vector-databases/

[^1_14]: https://www.linkedin.com/posts/evan-king-40072280_postgres-can-do-more-than-people-give-it-activity-7429917689353015296-mte5

[^1_15]: https://www.tigerdata.com/blog/combining-semantic-search-and-full-text-search-in-postgresql-with-cohere-pgvector-and-pgai

