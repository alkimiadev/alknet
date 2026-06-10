# Polyglot: Suitability Analysis & Comparisons

---

## 1. What Polyglot Is NOT

Before evaluating suitability, it's essential to understand what Polyglot **does not** do:

| NOT a... | Because |
|---|---|
| **Database driver** | No connection management, no query execution, no result set handling |
| **ORM** | No object-relational mapping, no model definitions, no active record pattern |
| **Migration tool** | No `CREATE TABLE` evolution management, no up/down migrations framework |
| **Type mapper** | No Rust type → SQL type mapping, no `FromRow` derives |
| **Connection pool** | No async I/O, no TCP connections, no TLS |
| **Query executor** | Never connects to a database; operates purely on SQL text |

**Polyglot is a SQL dialect transpiler.** It converts SQL strings between database dialects. Period.

---

## 2. Suitability Assessment for Multi-Database Storage Layer

### 2.1 What Polyglot CAN Do for a Multi-DB Project

| Use Case | Polyglot Support | Maturity |
|---|---|---|
| **SQL dialect translation** | ✅ Core purpose; 32 dialects with 100% test pass rate | Mature |
| **SQL pretty-printing** | ✅ Built-in format with guard rails | Mature |
| **SQL syntax validation** | ✅ Line/column error positions, error codes | Mature |
| **Schema-aware validation** | ✅ Table/column/type checking with `ValidationSchema` | Moderate |
| **Column lineage tracing** | ✅ `get_column_lineage()` for data lineage | Moderate |
| **OpenLineage payloads** | ✅ `RunEvent` and `DatasetFacet` generation | Early but functional |
| **Query builder** | ✅ Fluent API for SELECT/INSERT/UPDATE/DELETE | Usable but not as rich as query-builder-first libraries |
| **AST diff** | ✅ ChangeDistiller-based structural diff | Functional |
| **Logical planning** | ✅ Basic DAG plan extraction | Early stage |
| **Query optimization** | ✅ Column qualification, predicate pushdown, join elimination | Moderate |
| **Custom dialect registration** | ✅ `CustomDialectBuilder` for runtime extension | Functional |

### 2.2 What Polyglot CANNOT Do for a Multi-DB Project

| Need | Polyglot Support | Alternative |
|---|---|---|
| **Execute queries** | ❌ No | Use sqlx, diesel, or sea-orm |
| **Connection pooling** | ❌ No | Use deadpool, bb8, or sqlx built-in |
| **Async I/O** | ❌ Synchronous only | Wrap in `spawn_blocking()` |
| **Type-safe query building** | ⚠️ Partial (builder API returns strings) | Use diesel or sea-orm for compile-time checks |
| **Schema migration management** | ❌ No | Use diesel migrations, sqlx migrations, or refinery |
| **Row mapping / deserialization** | ❌ No | Use sqlx `FromRow`, diesel `Queryable` |
| **Runtime type mapping** | ⚠️ Limited (DataType enum, no Rust type bridge) | Build your own layer |
| **Database-specific DDL generation** | ⚠️ Parses/generates DDL but no migration framework | Use as a building block |
| **Transaction management** | ❌ No | Use sqlx or diesel |

### 2.3 Integration Pattern: Polyglot as a SQL Dialect Layer

The most natural integration pattern for a multi-database storage layer:

```
┌──────────────────────────────────────────────┐
│           Application Logic                   │
├──────────────────────────────────────────────┤
│         Query Builder / ORM Layer             │
│         (diesel / sea-orm / custom)            │
├──────────────────────┬───────────────────────┤
│                      │                        │
│   Polyglot Layer     │    Direct SQL          │
│   (transpile,        │    (no translation     │
│    validate,          │     needed)             │
│    format)            │                        │
├──────────────────────┴───────────────────────┤
│         Database Driver Layer                  │
│         (sqlx / diesel / tungstenite)          │
├──────────────────────────────────────────────┤
│    PostgreSQL    │    MySQL    │   SQLite      │
└──────────────────────────────────────────────┘
```

In this pattern, Polyglot sits **above** the database drivers, translating SQL from a canonical dialect to the target database's dialect before execution. It does **not** replace the drivers.

---

## 3. Comparison with Other Rust SQL Libraries

### 3.1 Feature Comparison Matrix

| Feature | **Polyglot** | **Diesel** | **SQLx** | **SeaORM** | **sqlparser-rs** |
|---|---|---|---|---|---|
| **Primary Purpose** | SQL transpilation | ORM / query builder | Async DB driver | Async ORM | SQL parsing |
| **SQL Parsing** | ✅ Full AST (200+ node types) | ✅ DSL-based | ❌ No | ❌ No | ✅ Full AST |
| **SQL Generation** | ✅ Multi-dialect | ✅ Via DSL | ❌ No | ❌ No | ⚠️ Limited |
| **Cross-dialect Transpilation** | ✅ 32 dialects | ❌ No | ❌ No | ❌ No | ❌ No |
| **Query Builder** | ⚠️ Fluent, string-based | ✅ Type-safe DSL | ❌ No | ✅ Type-safe | ❌ No |
| **Async I/O** | ❌ No (sync only) | ❌ Diesel 1.x is sync | ✅ Native async | ✅ Native async | ❌ No |
| **Type-safe Queries** | ❌ No (runtime) | ✅ Compile-time | ❌ No | ✅ Compile-time | ❌ No |
| **Connection Pool** | ❌ No | ❌ No (Diesel 2.x via r2d2) | ✅ Built-in | ✅ Built-in | ❌ No |
| **Migration Support** | ❌ No | ✅ Built-in | ❌ No | ✅ Built-in | ❌ No |
| **Database Execution** | ❌ No | ✅ Yes | ✅ Yes | ✅ Yes | ❌ No |
| **Schema Validation** | ✅ Via ValidationSchema | ✅ Compile-time | ❌ No | ⚠️ Limited | ❌ No |
| **Column Lineage** | ✅ Built-in | ❌ No | ❌ No | ❌ No | ❌ No |
| **AST Diff** | ✅ Built-in | ❌ No | ❌ No | ❌ No | ❌ No |
| **Dialects Supported** | 32 | 4 (PG, MySQL, SQLite, MSSQL) | N/A | N/A | 1 (ANSI SQL) |
| **License** | MIT | MIT/Apache-2.0 | MIT/Apache-2.0 | MIT | MIT/Apache-2.0 |
| **Maturity** | v0.4.4 (pre-1.0) | v2.2 (stable) | v0.8 (stable) | v1.1 (stable) | v0.49 (mature) |

### 3.2 Polyglot vs Diesel

| Aspect | Polyglot | Diesel |
|---|---|---|
| **Philosophy** | Parse any SQL → AST → generate any dialect | Type-safe DSL → SQL for specific databases |
| **Type Safety** | Runtime (string-based) | Compile-time (macro-based) |
| **Query Building** | `select(["col"]).from("t").where_(...)` → `Expression` AST | `schema::table::dsl::col.filter(...)` → SQL |
| **Dialect Breadth** | 32 dialects | 4 (PostgreSQL, MySQL, SQLite, MSSQL) |
| **Database Execution** | None (SQL text only) | Full CRUD with connection management |
| **Migrations** | None | Built-in migration framework |
| **When to use** | You need cross-dialect SQL translation, validation, lineage | You need type-safe queries with database execution |

**Verdict**: Polyglot and Diesel are **complementary**, not competing. Use Diesel for type-safe database interaction; use Polyglot when you need to translate SQL between dialects or analyze SQL without executing it.

### 3.3 Polyglot vs SQLx

| Aspect | Polyglot | SQLx |
|---|---|---|
| **Philosophy** | SQL manipulation without execution | Async database driver with compile-time query checking |
| **Async** | Synchronous only | Fully async |
| **Query Checking** | Runtime validation against schema | Compile-time `query!()` macro |
| **Database Support** | 32 dialects (parsing) | PostgreSQL, MySQL, SQLite (execution) |
| **When to use** | SQL transformation/analysis | Database interaction with async Rust |

**Verdict**: SQLx is for executing queries against databases. Polyglot is for transforming SQL text. They solve entirely different problems.

### 3.4 Polyglot vs SeaORM

| Aspect | Polyglot | SeaORM |
|---|---|---|
| **Philosophy** | SQL transpilation | Async ORM built on SQLx |
| **Async** | No | Yes |
| **Model Definition** | None | Entity models via macros |
| **Relationships** | None | Has-one, has-many, many-to-many |
| **When to use** | SQL dialect conversion | Database CRUD with relationships |

**Verdict**: Same as SQLx — complementary, not competing.

### 3.5 Polyglot vs sqlparser-rs

| Aspect | Polyglot | sqlparser-rs |
|---|---|---|
| **Parsing** | ✅ Full (200+ node types) | ✅ Full (ANSI SQL + some dialects) |
| **Generation** | ✅ Multi-dialect generation | ⚠️ Limited round-trip |
| **Transpilation** | ✅ Cross-dialect transforms | ❌ No |
| **Dialects** | 32 | Primarily ANSI SQL |
| **Validation** | ✅ With error positions | ❌ Parse errors only |
| **Builder** | ✅ Fluent API | ❌ No |
| **Lineage** | ✅ Built-in | ❌ No |
| **Diff** | ✅ Built-in | ❌ No |
| **Maturity** | v0.4.4 | v0.49 (more established) |

**Verdict**: sqlparser-rs is a mature parser for ANSI SQL. Polyglot offers significantly more: transpilation, 32 dialects, validation, lineage, diff, and a builder API. If you need dialect translation, Polyglot is the clear choice. If you only need ANSI SQL parsing and don't need generation/transpilation, sqlparser-rs may suffice with less overhead.

### 3.6 Polyglot vs Python sqlglot

| Aspect | Polyglot (Rust) | sqlglot (Python) |
|---|---|---|
| **Performance** | 8–19× faster (transpile), ~86× faster (generate) | Baseline |
| **Language** | Rust | Python |
| **Feature Parity** | ~95% of sqlglot's transpilation | Full feature set |
| **Optimizer** | Column qualification, predicate pushdown (moderate) | Full optimizer (column pruning, join elimination, etc.) |
| **Execution** | ❌ No | ⚠️ Limited (can execute against some engines) |
| **Test Compatibility** | 10,220+ sqlglot fixture cases at 100% | Original test suite |
| **Deployment** | Native binary / WASM / Python / Go | Python package |

**Verdict**: Polyglot is the performance-oriented port of sqlglot. It covers the core transpilation use case at near-full feature parity. The Python sqlglot has a more mature optimizer and some execution capabilities, but Polyglot is catching up rapidly (0.4.x adds lineage, OpenLineage, schema validation, and more).

---

## 4. Limitations and Gotchas

### 4.1 Current Limitations

| Limitation | Impact | Mitigation |
|---|---|---|
| **Pre-1.0 API** | Breaking changes possible between minor versions | Pin exact version in Cargo.toml |
| **No query execution** | Cannot run SQL against databases | Use alongside sqlx/diesel |
| **No async** | Blocking in async contexts | Wrap in `spawn_blocking()` |
| **No migration framework** | Cannot manage schema evolution | Use diesel migrations or refinery |
| **No Rust type mapping** | `DataType` enum doesn't map to Rust types | Build your own type bridge |
| **Builder returns Expression** | Builder doesn't produce type-safe queries | Accept runtime nature; pair with runtime validation |
| **Optimizer is early** | Limited optimization passes vs Python sqlglot | Most useful passes exist (qualify_columns, pushdown_predicates) |
| **WASM lacks `stacker`** | Deeply nested SQL may overflow stack in browser | Set format guard limits; consider web workers |
| **Custom dialects are global** | `CustomDialectBuilder` uses a global `RwLock` registry | Fine for most apps; not ideal for per-request isolation |
| **No prepared statement support** | Cannot generate `?` placeholders for parameterized queries | Build queries as strings; use sqlx for parameterization |

### 4.2 Gotchas

1. **`Dialect::get()` creates a new instance each call**: The `Dialect` struct bundles tokenizer + generator config + transformer. For hot loops, cache the `Dialect` instance rather than calling `Dialect::get()` repeatedly. (The overhead is minimal but non-zero.)

2. **Transpilation is not always invertible**: Some dialects have features that don't exist in others (e.g., BigQuery's `QUALIFY`, PostgreSQL's `ILIKE`, TSQL's `TOP`). Transpiling `A → B` and then `B → A` may lose information.

3. **Function transformation depth**: The transform pipeline processes per-node bottom-up. Some transformations require multi-pass processing (handled by `preprocess()`), but edge cases may require manual intervention.

4. **AST is not a stable serialization format**: The `Expression` enum and its inner structs may change between versions. If you serialize ASTs to JSON, expect breaking changes across minor versions.

5. **Feature flags are cumulative**: `transpile` implies `generate`, `openlineage` implies `semantic`, etc. For minimal builds, use `default-features = false` and select only what you need.

6. **Global custom dialect registry**: Custom dialects registered via `CustomDialectBuilder::register()` are stored in a global `RwLock<HashMap>`. This means they persist for the lifetime of the process and are visible across threads. Call `unregister_custom_dialect()` to remove them.

7. **Parser is permissive**: The parser accepts many SQL constructs that some databases reject. Validation (via `validate()` or `validate_with_schema()`) can catch some issues, but it's not a substitute for database-level error checking.

8. **No `?` placeholder generation**: Polyglot doesn't generate parameterized query placeholders. For prepared statements, you'll need to handle parameter binding yourself with your database driver.

9. **Schema validation requires manual schema definition**: The `ValidationSchema` struct must be populated manually — there's no automatic schema introspection from a live database.

---

## 5. Production-Readiness Assessment

### 5.1 Strengths

| Area | Rating | Notes |
|---|---|---|
| **Transpilation accuracy** | ⭐⭐⭐⭐⭐ | 10,220+ fixture cases at 100% pass rate |
| **Performance** | ⭐⭐⭐⭐⭐ | 8–19× faster than Python sqlglot |
| **Dialect coverage** | ⭐⭐⭐⭐⭐ | 32 dialects covering all major databases |
| **API ergonomics** | ⭐⭐⭐⭐ | Clean public API; builder is pleasant |
| **Error reporting** | ⭐⭐⭐⭐ | Line/column/byte-offset positions |
| **WASM support** | ⭐⭐⭐⭐ | Full feature set in browser |
| **Multi-language bindings** | ⭐⭐⭐⭐⭐ | Rust, TypeScript, Python, Go, C FFI |
| **Documentation** | ⭐⭐⭐ | Rust API docs exist; could use more guides |
| **Test coverage** | ⭐⭐⭐⭐⭐ | 18,745 test cases |
| **Fuzzing** | ⭐⭐⭐⭐ | Supported via `cargo fuzz` |

### 5.2 Risks

| Risk | Severity | Mitigation |
|---|---|---|
| **Pre-1.0 breaking changes** | Medium | Pin version; monitor CHANGELOG |
| **Single maintainer** | Medium | Code is well-structured; community could fork |
| **Limited optimizer** | Low | Core passes exist; Python sqlglot is reference |
| **No query execution** | Low (by design) | Combine with sqlx/diesel |
| **WASM stack limits** | Low | Set guard rails; use web workers |

### 5.3 Overall Assessment

**Polyglot is production-viable for SQL transpilation and analysis tasks**, with caveats:

- ✅ **Use for**: SQL dialect translation, SQL linting/validation, column lineage, pretty-printing, AST analysis, cross-database query migration
- ⚠️ **Use with caution for**: Query building (no type safety), optimization (partial coverage)
- ❌ **Don't use for**: Database execution, connection management, migrations, type-safe queries

For a multi-database storage layer, the recommended pattern is:

```
Application → Polyglot (transpile SQL to target dialect) → sqlx/diesel (execute)
```

---

## 6. Recommendation

### When to Adopt Polyglot

1. **You need to support multiple database backends with different SQL dialects** and want to write queries once in a canonical dialect, then transpile to the target at runtime.
2. **You need SQL validation or analysis** (lineage, schema checking) without executing queries.
3. **You need SQL pretty-printing or formatting** with configurable guard rails.
4. **You need column lineage tracking** for data governance or OpenLineage integration.
5. **You need to parse and analyze SQL** in a Rust/WASM/Python/Go context without connecting to a database.

### When NOT to Adopt Polyglot

1. **You need type-safe query building** — use Diesel or SeaORM instead.
2. **You need async database execution** — use SQLx or SeaORM instead.
3. **You need schema migrations** — use Diesel migrations, sqlx migrations, or Refinery instead.
4. **You only need PostgreSQL** (or a single dialect) — a simpler parser may suffice.
5. **You need Rust type → SQL type mapping** — Polyglot doesn't provide this.

### Suggested Adoption Strategy

For a multi-database storage layer:

1. **Use Polyglot for SQL transpilation**: Write queries in a canonical dialect (e.g., PostgreSQL-compatible), transpile to the target dialect at runtime.
2. **Use SQLx for database execution**: Handle connections, pooling, and async I/O.
3. **Use Polyglot for validation**: Validate user-provided SQL before execution.
4. **Use Polyglot for lineage**: Trace column flow for data governance.
5. **Build a thin integration layer** that combines Polyglot's transpilation with SQLx's execution.

---

## References

- <https://github.com/tobilg/polyglot> — Main repository
- <https://crates.io/crates/polyglot-sql> — Rust crate (v0.4.4)
- <https://docs.rs/polyglot-sql/latest/polyglot_sql/> — Rust API docs
- <https://github.com/tobymao/sqlglot> — Python inspiration
- <https://lib.rs/crates/polyglot-sql> — Package metadata
- Local source: `/workspace/polyglot/`