<img src="https://r2cdn.perplexity.ai/pplx-full-logo-primary-dark%402x.png" style="height:64px;margin-right:32px"/>

# rust support for PostGres

Rust has solid, production-grade PostgreSQL support across both low-level drivers and higher-level ORMs/libraries.[^1_1][^1_2]

## Core drivers

- **tokio-postgres**: Native async PostgreSQL client, designed to work with Tokio; gives you fine-grained control over connections, prepared statements, and type mappings.[^1_3][^1_1]
- **postgres** crate: Sync wrapper around `tokio-postgres`, convenient if your app is not async and you want a blocking API.[^1_4][^1_1]
- **postgres-types**, **postgres-openssl**, **postgres-native-tls**: Type conversions and TLS integration that plug into the above clients.[^1_1]

These are the foundational crates most other libraries build on.

## Async, raw SQL libraries

- **SQLx**: Async, runtime-agnostic (Tokio, async-std) with compile-time query checking using a build-time database connection; supports Postgres, MySQL, SQLite, MSSQL.[^1_2][^1_5]
- **tokio-postgres** + your own query layer: Option when you want to stay very close to the wire protocol but still be async.[^1_5][^1_1]

SQLx is often chosen instead of an ORM when teams want control and performance while still getting safety from compile-time SQL validation.[^1_2]

## ORMs / higher-level abstractions

- **Diesel**: Oldest and most mature Rust ORM with strong compile-time query validation and excellent Postgres support; sync by default, with `diesel-async` for async runtimes.[^1_6][^1_2]
- **SeaORM**: Async-first ORM built on SQLx; entity/ActiveModel pattern, designed for modern web stacks like Axum/Actix.[^1_2]
- **rbatis**: Async, macro-based, MyBatis-style dynamic SQL; smaller ecosystem but interesting if you like MyBatis semantics.[^1_2]

A common rule of thumb in 2026 is: Diesel for maximum compile-time safety and performance, SeaORM for ergonomic async CRUD APIs, SQLx when you want raw SQL with strong checks.[^1_7][^1_2]

## Special-purpose and ecosystem crates

- **pgwire**: Implements the PostgreSQL wire protocol for building proxies or custom Postgres-compatible servers, not a general app driver.[^1_8]
- Various helper crates (e.g., `sal_postgresclient`) wrap clients with pooling, env-driven config, or scripting support, but these are more niche.[^1_9]


## Practical starting points

If you’re starting a new Postgres-backed Rust service today:

- **Async web API (Axum/Actix/Tonic)**:
    - Prefer **SQLx** or **SeaORM** for async-first ergonomics.[^1_5][^1_2]
- **Heavy data model, strong type safety, mostly sync or controlled async**:
    - Use **Diesel** (+ `diesel-async` if needed).[^1_6][^1_2]
- **Driver-level control, custom pooling/protocol work**:
    - Use **tokio-postgres** directly.[^1_3][^1_1]

If you share a bit about your runtime (Tokio vs others), sync vs async, and how much ORM you want, I can sketch a minimal idiomatic setup for that stack.
<span style="display:none">[^1_10][^1_11][^1_12][^1_13][^1_14][^1_15]</span>

<div align="center">⁂</div>

[^1_1]: https://lib.rs/crates/tokio-postgres

[^1_2]: https://www.linkedin.com/posts/rahul-chauhan-508968252_rustlang-database-orm-activity-7416120222564229120-pgb0

[^1_3]: https://github.com/rust-postgres/rust-postgres

[^1_4]: https://docs.rs/postgres

[^1_5]: https://www.youtube.com/watch?v=1Ir77CZaRDU

[^1_6]: https://users.rust-lang.org/t/recommended-postgres-bindings/100838

[^1_7]: https://www.youtube.com/watch?v=YkgaMnheiDM

[^1_8]: https://crates.io/crates/pgwire

[^1_9]: https://docs.rs/sal-postgresclient

[^1_10]: https://crates.io/crates/postgres

[^1_11]: https://www.reddit.com/r/rust/comments/uhxrs8/what_is_your_goto_database_crate_for_postgresql/

[^1_12]: https://stackoverflow.com/questions/67851017/rust-postgres-synchronous-driver-called-from-async-main-causes-cannot-start-a

[^1_13]: https://www.reddit.com/r/rust/comments/1lqinlw/best_orm/

[^1_14]: https://docs.rs/crate/async-postgres/latest

[^1_15]: https://www.shuttle.dev/blog/2024/01/16/best-orm-rust

