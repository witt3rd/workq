FROM ghcr.io/pgmq/pg17-pgmq:latest

USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends postgresql-17-pgvector \
    && rm -rf /var/lib/apt/lists/*
USER postgres
