# Polyglot: Research Overview

**Library**: `polyglot-sql` (Rust crate) / `@polyglot-sql/sdk` (TypeScript/WASM) / `polyglot-sql` (Python)  
**Repository**: <https://github.com/tobilg/polyglot>  
**Current Version**: 0.4.4 (as of 2026-06-03)  
**License**: MIT (+ sqlglot MIT for test fixtures)  
**Author**: Tobias G. (tobilg)  
**Inspiration**: Python [sqlglot](https://github.com/tobymao/sqlglot) by Toby Mao  

---

## 1. What Is Polyglot?

Polyglot is a **SQL transpiler** — it parses SQL from one database dialect into an AST, and generates SQL for a different dialect. It is **not** a database driver, ORM, query executor, or connection pool. Its core purpose is **dialect-agnostic SQL manipulation**: parse, transform, validate, format, and transpile SQL across 32+ database dialects.

### Key Capabilities

| Capability | Description |
|---|---|
| **Parse** | Convert SQL string → typed AST with 200+ expression node types |
| **Generate** | Convert AST → SQL string for any supported dialect |
| **Transpile** | Convert SQL from dialect A → dialect B in one call |
| **Format** | Pretty-print SQL with configurable guard rails |
| **Build** | Construct SQL programmatically via a fluent builder API |
| **Validate** | Syntax + semantic validation with error positions |
| **Lineage** | Trace column lineage through queries; generate OpenLineage payloads |
| **Diff** | AST-aware diff between two SQL expressions |
| **Traverse** | DFS/BFS iterators, predicate queries, and transforms on the AST |

### Supported Dialects (32)

Athena, BigQuery, ClickHouse, CockroachDB, Databricks, Doris, Dremio, Drill, Druid, DuckDB, Dune, Exasol, Fabric, Hive, Materialize, MySQL, Oracle, PostgreSQL, Presto, Redshift, RisingWave, SingleStore, Snowflake, Solr, Spark, SQLite, StarRocks, Tableau, Teradata, TiDB, Trino, TSQL

Plus a `Generic` dialect for standard SQL.

### Language Bindings

| Binding | Package | Delivery |
|---|---|---|
| **Rust** | `polyglot-sql` on crates.io | Native Rust crate |
| **TypeScript/WASM** | `@polyglot-sql/sdk` on npm | WASM module + JS wrapper |
| **Python** | `polyglot-sql` on PyPI | PyO3 native extension |
| **Go** | `github.com/tobilg/polyglot/packages/go` | PureGo wrapper over C FFI |
| **C FFI** | Built from `polyglot-sql-ffi` | `.so` / `.dylib` / `.dll` + `.a` / `.lib` + header |

---

## 2. Core Philosophy & Design Principles

1. **Pipeline architecture**: SQL → Tokenize → Parse → AST → Transform → Generate → SQL string. Each stage is independently configurable per dialect.

2. **Ported from Python sqlglot**: The Rust implementation is a faithful port of the Python `sqlglot` library, maintaining compatibility with its test fixtures (10,220+ fixture cases at 100% pass rate). The architecture, expression types, transformation rules, and dialect behaviors mirror the Python original.

3. **No runtime database connection**: Polyglot never connects to a database. It operates purely on SQL strings and ASTs. This makes it safe for sandboxed environments (WASM, serverless) and suitable for build-time / CI-time SQL analysis.

4. **Feature-gated compilation**: Each dialect is behind a Cargo feature flag (`dialect-postgresql`, `dialect-mysql`, etc.), so users compiling for constrained targets (WASM) can include only what they need. The `default` feature set includes everything.

5. **Stack safety**: The `stacker` feature (default-on for native builds) grows the stack on deeply nested inputs, preventing stack overflow from pathological SQL. WASM builds opt out since `stacker` doesn't work there.

6. **Guard rails**: Format/guard options limit input size (16 MiB default), token count (1M), AST node count (1M), and set-operation chain depth (256) to prevent resource exhaustion.

7. **Performance-first**: Built in Rust for speed. Benchmarks show 8–19× speedup over the Python `sqlglot` for transpilation, with generation at ~86× faster. The WASM build enables near-native performance in browsers.

---

## 3. How It Differs from Database Abstraction Layers

**Critical distinction**: Polyglot is a **SQL dialect transpiler**, not a database abstraction layer. It does not:

- Connect to databases
- Execute queries
- Manage connection pools
- Handle migrations (no `CREATE TABLE` schema evolution management)
- Map Rust types to database types
- Provide an ORM-like interface
- Handle async I/O

Instead, it focuses purely on **SQL text manipulation**: parsing, analyzing, transforming, and generating SQL strings. This makes it complementary to (not competing with) libraries like Diesel, SQLx, or SeaORM.

---

## 4. Performance Characteristics

From the project's benchmark suite (polyglot-sql v0.1.2 vs sqlglot v28.10.1):

| Operation | Speedup Range |
|---|---|
| Parse (SQL → AST) | 10–13× faster |
| Generate (AST → SQL) | 77–101× faster |
| Roundtrip (parse → generate → re-parse) | 13–15× faster |
| Transpile (full cross-dialect) | 1.6× (simple) to 19× (complex BigQuery→Snowflake) |
| Geometric mean | **8.70×** |

Parse benchmarks (v0.4.x, native Rust):

| Query | Mean |
|---|---|
| short (SELECT a, b, c) | 51.28 μs |
| medium (5 cols, JOIN, GROUP BY) | 259.61 μs |
| complex (3 CTEs, subquery) | 268.59 μs – 1.03 ms |

---

## 5. Project Maturity Indicators

| Indicator | Status |
|---|---|
| **Version** | 0.4.4 (pre-1.0, active development) |
| **Test coverage** | 18,745 test cases at 100% pass rate |
| **crates.io downloads** | ~4,738 total (as of mid-2026) |
| **Dependent crates** | 2 (via entdb) |
| **Release cadence** | Frequent patch releases (0.4.2, 0.4.3, 0.4.4 in quick succession) |
| **Source code size** | ~241K lines of Rust in core crate |
| **Fuzzing** | Supported via `cargo +nightly fuzz` |
| **CI** | Full test suite + FFI + Python + WASM |
| **Documentation** | Rust API docs (docs.rs), TypeScript docs, Python docs, playground |
| **Breaking changes** | Possible before 1.0; semver suggests API instability |

---

## 6. License

- **MIT License** for the Polyglot code itself
- **sqlglot MIT License** for the test fixtures derived from the Python project
- Both are permissive, suitable for commercial use

---

## References

- <https://github.com/tobilg/polyglot> — Main repository
- <https://crates.io/crates/polyglot-sql> — Rust crate on crates.io
- <https://www.npmjs.com/package/@polyglot-sql/sdk> — TypeScript SDK on npm
- <https://pypi.org/project/polyglot-sql/> — Python bindings on PyPI
- <https://docs.rs/polyglot-sql/latest/polyglot_sql/> — Rust API documentation
- <https://polyglot-playground.gh.tobilg.com/> — Interactive playground
- <https://github.com/tobymao/sqlglot> — Original Python inspiration