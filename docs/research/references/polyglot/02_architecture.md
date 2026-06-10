# Polyglot: Architecture Deep Dive

---

## 1. Workspace Structure

The repository is organized as a Cargo workspace with 5 crates and supporting packages:

```
polyglot/
├── crates/
│   ├── polyglot-sql/                    # Core Rust library (~241K LOC)
│   │   └── src/
│   │       ├── lib.rs                   # Public API, top-level functions
│   │       ├── tokens.rs               # Tokenizer (lexer)
│   │       ├── parser.rs                # Recursive-descent parser (~62K LOC)
│   │       ├── expressions.rs           # AST node types (~15K LOC)
│   │       ├── generator.rs             # SQL code generator (~39K LOC)
│   │       ├── dialects/                # 33 dialect implementations
│   │       │   ├── mod.rs              # Dialect trait, Dialect struct, CustomDialectBuilder
│   │       │   ├── generic.rs          # Base/standard SQL dialect
│   │       │   ├── postgres.rs         # PostgreSQL (~1.9K LOC)
│   │       │   ├── mysql.rs            # MySQL
│   │       │   ├── sqlite.rs           # SQLite
│   │       │   ├── bigquery.rs         # BigQuery
│   │       │   ├── ... (32 total)
│   │       ├── builder.rs              # Fluent query builder API
│   │       ├── transforms.rs           # Cross-dialect transform functions
│   │       ├── validation.rs           # Syntax + semantic validation
│   │       ├── schema.rs              # Schema representation
│   │       ├── scope.rs               # Scope analysis
│   │       ├── resolver.rs            # Column resolution
│   │       ├── lineage.rs             # Column lineage tracking
│   │       ├── openlineage.rs          # OpenLineage payload generation
│   │       ├── diff.rs                # AST diff (ChangeDistiller algorithm)
│   │       ├── planner.rs             # Logical query plan
│   │       ├── optimizer/              # Query optimizer modules
│   │       │   ├── annotate_types.rs  # Type annotation
│   │       │   ├── qualify_columns.rs # Column qualification
│   │       │   ├── qualify_tables.rs   # Table qualification
│   │       │   ├── pushdown_predicates.rs
│   │       │   ├── pushdown_projections.rs
│   │       │   ├── eliminate_joins.rs
│   │       │   ├── eliminate_ctes.rs
│   │       │   ├── simplify.rs
│   │       │   └── ...
│   │       ├── traversal.rs            # DFS/BFS visitors, AST predicates
│   │       ├── ast_transforms.rs       # AST manipulation utilities
│   │       ├── error.rs               # Error types
│   │       └── time.rs                # Time format conversion
│   ├── polyglot-sql-function-catalogs/  # Optional dialect function catalogs
│   ├── polyglot-sql-wasm/              # WASM bindings (wasm-pack)
│   ├── polyglot-sql-ffi/               # C FFI bindings (cbindgen)
│   └── polyglot-sql-python/            # Python bindings (PyO3 + maturin)
├── packages/
│   ├── sdk/                            # TypeScript SDK (@polyglot-sql/sdk)
│   ├── go/                             # Go SDK (PureGo wrapper over FFI)
│   ├── documentation/                  # TypeScript API docs site
│   ├── playground/                      # Browser playground (React 19, Vite)
│   └── python-docs/                     # Python API docs
├── examples/
│   ├── rust/                           # Rust usage example
│   ├── typescript/                     # TypeScript SDK example
│   └── c/                              # C FFI usage example
└── tools/
    ├── sqlglot-compare/                # Fixture extraction & comparison
    └── bench-compare/                  # Performance benchmarks
```

---

## 2. Data Flow Pipeline

```
┌──────────────────────────────────────────────────────────────────────┐
│                        SQL String (source dialect)                   │
└──────────────────────────┬──────────────────────────────────────────┘
                           │
                           ▼
┌──────────────────────────────────────────────────────────────────────┐
│                        Tokenizer (tokens.rs)                         │
│  • Dialect-specific lexing rules (quotes, comments, keywords)        │
│  • Configurable via TokenizerConfig per dialect                      │
│  • Produces Vec<Token> with type, text, and Span (line/col/offset) │
└──────────────────────────┬──────────────────────────────────────────┘
                           │
                           ▼
┌──────────────────────────────────────────────────────────────────────┐
│                    Parser (parser.rs, ~62K LOC)                      │
│  • Recursive-descent with precedence climbing                       │
│  • Dialect-aware parsing (custom keywords, syntax rules)            │
│  • Produces Expression AST tree                                     │
│  • Stack safety via `stacker` feature (default-on)                  │
└──────────────────────────┬──────────────────────────────────────────┘
                           │
                           ▼
┌──────────────────────────────────────────────────────────────────────┐
│                    Expression AST (expressions.rs)                    │
│  • Single tagged enum with 150+ variants                            │
│  • Each variant has its own struct (Select, Insert, Function, etc.) │
│  • Box<Variant> keeps enum size to 2 words (tag + pointer)          │
│  • Serializable via serde (derive Serialize/Deserialize)             │
│  • Optional TypeScript type generation via `ts-rs` feature flag      │
└──────────────────────────┬──────────────────────────────────────────┘
                           │
                      ┌────┴────┐
                      │         │
            ┌─────────┘         └──────────┐
            │                               │
            ▼                               ▼
┌────────────────────────┐    ┌────────────────────────────────────┐
│  Transform Pipeline     │    │  Semantic / Analysis Modules       │
│  (transpile path)      │    │  • validation.rs → syntax checks   │
│                        │    │  • schema.rs → column/type lookup   │
│  1. preprocess()       │    │  • scope.rs → scope analysis        │
│     (whole-tree rewrites│    │  • resolver.rs → column resolution  │
│      like eliminate_   │    │  • lineage.rs → column lineage      │
│      qualify)           │    │  • openlineage.rs → OL payloads    │
│                        │    │  • optimizer/ → query optimization   │
│  2. transform_expr()   │    │  • diff.rs → AST diff               │
│     (per-node rewrites  │    │  • planner.rs → logical plan DAG    │
│      per dialect)       │    │  • traversal.rs → DFS/BFS visitors   │
│                        │    │
│  3. Generator          │    │
│     (AST → SQL string) │    │
└───────────┬────────────┘    └────────────────────────────────────┘
            │
            ▼
┌──────────────────────────────────────────────────────────────────────┐
│                     SQL String (target dialect)                      │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 3. Core Abstractions

### 3.1 Expression AST

The central type is `Expression`, a large tagged enum with one variant per SQL construct:

```rust
pub enum Expression {
    // Literals
    Literal(Box<Literal>),
    Boolean(BooleanLiteral),
    Null(Null),

    // Identifiers
    Identifier(Identifier),
    Column(Box<Column>),
    Table(Box<TableRef>),
    Star(Star),

    // Queries
    Select(Box<Select>),
    Union(Box<Union>),
    Intersect(Box<Intersect>),
    Except(Box<Except>),
    Subquery(Box<Subquery>),

    // DML
    Insert(Box<Insert>),
    Update(Box<Update>),
    Delete(Box<Delete>),
    Copy(Box<CopyStmt>),

    // Binary/Unary operators
    And(Box<BinaryOp>),
    Or(Box<BinaryOp>),
    Add(Box<BinaryOp>),
    Eq(Box<BinaryOp>),
    // ... 30+ operator variants

    // Functions
    Function(Box<Function>),
    AggregateFunction(Box<AggregateFunction>),
    WindowFunction(Box<WindowFunction>),

    // Clauses
    From(Box<From>),
    Join(Box<Join>),
    Where(Box<Where>),
    OrderBy(Box<OrderBy>),
    // ...

    // ~150 total variants
}
```

Key design choices:
- **Boxed variants**: Most variants wrap their payload in `Box` to keep `size_of::<Expression>()` at 2 words (16 bytes on 64-bit).
- **Serde support**: `#[derive(Serialize, Deserialize)]` for JSON serialization across FFI/WASM boundaries.
- **TypeScript types**: Optional `ts-rs` feature generates TypeScript interfaces.
- **Convenience methods**: `Expression::column()`, `Expression::number()`, `Expression::sql()`, `Expression::sql_for()`.

### 3.2 DialectType Enum

```rust
pub enum DialectType {
    Generic, PostgreSQL, MySQL, BigQuery, Snowflake, DuckDB, SQLite,
    Hive, Spark, Trino, Presto, Redshift, TSQL, Oracle, ClickHouse,
    Databricks, Athena, Teradata, Doris, StarRocks, Materialize,
    RisingWave, SingleStore, CockroachDB, TiDB, Druid, Solr, Tableau,
    Dune, Fabric, Drill, Dremio, Exasol, DataFusion,
}
```

- Implements `FromStr` with aliases (e.g., `"mssql"` → `TSQL`, `"cockroach"` → `CockroachDB`)
- Each variant maps to a feature-gated dialect module
- Custom dialects can be registered at runtime via `CustomDialectBuilder`

### 3.3 DialectImpl Trait

```rust
pub trait DialectImpl {
    fn dialect_type(&self) -> DialectType;
    fn tokenizer_config(&self) -> TokenizerConfig { /* default */ }
    fn generator_config(&self) -> GeneratorConfig { /* default */ }
    fn generator_config_for_expr(&self, _expr: &Expression) -> GeneratorConfig { /* default */ }
    fn transform_expr(&self, expr: Expression) -> Result<Expression> { Ok(expr) }
    fn preprocess(&self, expr: Expression) -> Result<Expression> { Ok(expr) }
}
```

Each dialect implements this trait to provide:
1. **Tokenizer config**: Identifier quoting characters, string delimiters, keyword overrides, comment styles, hex number support
2. **Generator config**: 30+ flags controlling SQL output (identifier quote style, function casing, `LIMIT` vs `TOP` vs `FETCH FIRST`, etc.)
3. **Per-node transform**: Dialect-specific expression rewrites (e.g., PostgreSQL transforms `IFNULL` → `COALESCE`, SQLite transforms `TRY_CAST` → `CAST`)
4. **Whole-tree preprocess**: Structural rewrites that need full-tree context (e.g., eliminating `QUALIFY` for dialects that don't support it)

### 3.4 Dialect Struct (High-Level API)

```rust
pub struct Dialect {
    dialect_type: DialectType,
    tokenizer: Tokenizer,
    generator_config: Arc<GeneratorConfig>,
    transformer: Box<dyn Fn(Expression) -> Result<Expression> + Send + Sync>,
    generator_config_for_expr: Option<Box<dyn Fn(&Expression) -> GeneratorConfig + Send + Sync>>,
    custom_preprocess: Option<Box<dyn Fn(Expression) -> Result<Expression> + Send + Sync>>,
}
```

The `Dialect` struct bundles all dialect-specific state and provides the primary API:

```rust
// Parse SQL
let ast = dialect.parse("SELECT 1")?;

// Generate SQL from AST
let sql = dialect.generate(&ast[0])?;

// Transpile between dialects
let results = dialect.transpile("SELECT IFNULL(a,b) FROM t", DialectType::PostgreSQL)?;

// Tokenize
let tokens = dialect.tokenize("SELECT 1")?;
```

### 3.5 CustomDialectBuilder

For runtime-extensible dialect support:

```rust
use polyglot_sql::dialects::{CustomDialectBuilder, Dialect, DialectType};
use polyglot_sql::generator::NormalizeFunctions;

// Register a custom dialect inheriting from PostgreSQL
CustomDialectBuilder::new("my_postgres")
    .based_on(DialectType::PostgreSQL)
    .generator_config_modifier(|gc| {
        gc.normalize_functions = NormalizeFunctions::Lower;
    })
    .register()?;

let d = Dialect::get_by_name("my_postgres").unwrap();
// Use like any built-in dialect
```

---

## 4. Dialect Implementation Details

### 4.1 PostgreSQL (`postgres.rs`, ~1,879 LOC)

**Tokenizer:**
- `$$` string literals (dollar-quoting)
- Double-quote identifier quoting
- Nested block comments
- `EXEC` treated as generic command

**Generator config highlights:**
- `identifier_quote: '"'` (double quotes)
- `single_string_interval: true` (`INTERVAL '1 day'`)
- `parameter_token: "$"` (`$1`, `$2` placeholders)
- `supports_select_into: true`
- `supports_window_exclude: true`
- `can_implement_array_any: true`

**Transform examples:**
- `IFNULL(a, b)` → `COALESCE(a, b)`
- `RAND()` → `RANDOM()`
- `DATEDIFF(day, a, b)` → `CAST(b - a AS INT)` (date subtraction)
- `JSON_EXTRACT(a, '$.x')` → `a #> '{x}'` (arrow syntax)
- `JSON_EXTRACT_SCALAR(a, '$.x')` → `a #>> '{x}'`
- `DATE_ADD` / `DATE_SUB` → `+` / `-` interval arithmetic
- Type mappings: `TINYINT` → `SMALLINT`, `FLOAT` → `REAL`, `DOUBLE` → `DOUBLE PRECISION`
- `ILIKE` preserved (native PostgreSQL)
- `RegexpLike` → `~` operator, `RegexpILike` → `~*` operator

### 4.2 SQLite (`sqlite.rs`, ~750 LOC)

**Tokenizer:**
- Supports `"`, `[`, `` ` `` as identifier quote characters
- No nested comments
- Hex number literals (`0xCC`)

**Generator config:**
- `identifier_quote: '"'` (double quotes)
- `supports_table_alias_columns: false`
- `json_key_value_pair_sep: ","` (comma-style `JSON_OBJECT`)

**Transform examples:**
- `NVL(a, b)` → `IFNULL(a, b)`
- `TRY_CAST(x AS t)` → `CAST(x AS t)` (no try-cast)
- `RANDOM()` → function
- `ILIKE` → `LOWER(left) LIKE LOWER(right)` (no native ILIKE)
- `CountIf(cond)` → `SUM(IIF(cond, 1, 0))`
- `CEIL(x)` → function form
- `DATE_TRUNC(unit, col)` → various strftime patterns
- `DATE_DIFF` → `juliandiff` patterns

### 4.3 MySQL (`mysql.rs`)

**Tokenizer:** Backtick identifiers, `#` comments
**Generator:** Backtick quoting, `LIMIT` syntax, `CONCAT()` instead of `||`
**Transforms:** `COALESCE(a,b)` ← `IFNULL(a,b)`, `||` → `CONCAT()` (string concat), etc.

### 4.4 BigQuery (`bigquery.rs`)

**Tokenizer:** Backtick identifiers, `QUALIFY` keyword
**Generator:** Backtick quoting, `STRUCT` types, `QUALIFY` clause, `DATE_DIFF` syntax
**Transforms:** Complex date/timestamp function mappings, `UNNEST` handling, `APPROX_COUNT_DISTINCT` → `APPROX_COUNT_DISTINCT`

### 4.5 How Transpilation Works

The full transpilation pipeline:

```
Input SQL (source dialect)
      │
      ▼
Source Dialect Tokenizer
      │
      ▼
Parser (dialect-aware)
      │
      ▼
Expression AST
      │
      ▼
Source Dialect::preprocess()      ← whole-tree rewrites
      │
      ▼
Source Dialect::transform_expr()   ← per-node rewrites (recursive, bottom-up)
      │
      ▼
Normalized AST
      │
      ▼
Target Dialect Generator
      │
      ▼
Output SQL (target dialect)
```

The transform pipeline uses an explicit task stack (not recursive calls) for the hot paths to avoid stack overflow. The `stacker` crate provides additional stack-growth protection.

Key cross-dialect transforms include:
- Function renaming: `IFNULL` ↔ `COALESCE` ↔ `NVL`, `DATEDIFF` ↔ date arithmetic, `STRING_AGG` ↔ `GROUP_CONCAT`
- Type mapping: `TINYINT` ↔ `SMALLINT`, `FLOAT` ↔ `REAL`, `JSON` ↔ `JSONB`
- Syntax conversion: `LIMIT` ↔ `TOP` ↔ `FETCH FIRST`, `||` (concat) ↔ `CONCAT()`, `SELECT INTO` ↔ `CREATE TABLE AS`
- Boolean handling: `BOOL_AND`/`BOOL_OR` ↔ `MIN`/`MAX`-over-`CASE`
- JSON operators: `JSON_EXTRACT` ↔ `#>`/`#>>` ↔ `->`/`->>` (PostgreSQL arrow syntax)

---

## 5. Fluent Builder API

The builder module (`builder.rs`, ~3.3K LOC) provides a type-safe, ergonomic way to construct SQL expressions without string interpolation:

```rust
use polyglot_sql::builder::*;

// SELECT id, name FROM users WHERE age > 18 ORDER BY name LIMIT 10
let expr = select(["id", "name"])
    .from("users")
    .where_(col("age").gt(lit(18)))
    .order_by(["name"])
    .limit(10)
    .build();

// INSERT
let ins = insert_into("users")
    .columns(["id", "name"])
    .values([lit(1), lit("Alice")])
    .build();

// CASE expression
let expr = case()
    .when(col("x").gt(lit(0)), lit("positive"))
    .else_(lit("non-positive"))
    .build();

// Set operations
let expr = union_all(
    select(["id"]).from("a"),
    select(["id"]).from("b"),
).order_by(["id"]).limit(5).build();
```

Expression helpers:
- `col("users.id")` — column reference (splits on last `.`)
- `lit(42)`, `lit("hello")`, `lit(3.14)`, `lit(true)` — literals
- `func("COALESCE", [col("a"), col("b")])` — function calls
- Operator chain: `col("age").gte(lit(18)).and(col("status").eq(lit("active")))`

The builder generates an `Expression` AST that can then be serialized to any dialect via `generate()`.

---

## 6. Validation and Schema-Aware Analysis

### 6.1 Syntax Validation

```rust
use polyglot_sql::{validate, DialectType};

let result = validate("SELECT * FORM users", DialectType::Generic);
// result.valid == false
// result.errors contain line/column/message/error codes
```

Error codes:
- `E001` — Syntax error
- `E002` — Tokenization error
- `E003` — Parse error
- `E004` — Invalid expression (not a valid statement)
- `E005` — Trailing comma in strict mode

### 6.2 Schema-Aware Validation

```rust
use polyglot_sql::{
    validate_with_schema, DialectType, SchemaColumn, SchemaTable,
    SchemaValidationOptions, ValidationSchema,
};

let schema = ValidationSchema {
    strict: Some(true),
    tables: vec![
        SchemaTable {
            name: "users".into(),
            columns: vec![
                SchemaColumn { name: "id".into(), data_type: "integer".into(), nullable: Some(false), primary_key: true, unique: false, references: None },
                SchemaColumn { name: "email".into(), data_type: "varchar".into(), nullable: Some(false), primary_key: false, unique: true, references: None },
            ],
            // ...
        },
    ],
};

let opts = SchemaValidationOptions { check_types: true, check_references: true, strict: None, semantic: true };
let result = validate_with_schema("SELECT id FROM users WHERE email = 1", DialectType::Generic, &schema, &opts);
// result.valid == false (type mismatch: email is varchar, compared to integer)
```

Schema-aware error codes:
- `E200`/`E201` — Unknown table/column
- `E210`–`E217`, `W210`–`W216` — Type checks
- `E220`, `E221`, `W220`, `W221`, `W222` — Reference/FK checks

### 6.3 Function Catalogs

Optional feature-gated function catalogs (currently ClickHouse and DuckDB) provide known function signatures for semantic type checking:

```toml
polyglot-sql = { version = "0.4", features = ["function-catalog-clickhouse"] }
```

---

## 7. Column Lineage & OpenLineage

### 7.1 Column Lineage

Trace how columns flow through a query:

```rust
use polyglot_sql::{parse, DialectType};
use polyglot_sql::lineage::get_column_lineage;

let ast = parse("SELECT a + b AS total FROM t", DialectType::Generic).unwrap();
let lineage = get_column_lineage(&ast[0], /* schema */ None, DialectType::Generic);
// lineage tells you that "total" depends on columns "a" and "b" from table "t"
```

### 7.2 OpenLineage Payload Generation

```rust
use polyglot_sql::openlineage::{generate_run_event, OpenLineageOptions, OpenLineageDatasetId};

let opts = OpenLineageOptions {
    dialect: DialectType::PostgreSQL,
    producer: "my-app".into(),
    dataset_namespace: Some("mydb".into()),
    // ...
};
let event = generate_run_event("SELECT * FROM users", &opts)?;
// event is a JSON-serializable OpenLineage RunEvent with columnLineage facets
```

---

## 8. Error Handling

### 8.1 Error Types

```rust
pub enum Error {
    Tokenize { message: String, line: usize, column: usize, start: usize, end: usize },
    Parse { message: String, line: usize, column: usize, start: usize, end: usize },
    Generate(String),
    Unsupported { feature: String, dialect: String },
    Syntax { message: String, line: usize, column: usize, start: usize, end: usize },
    Internal(String),
}
```

All position-bearing errors include:
- `line` — 1-based line number
- `column` — 1-based column number
- `start` / `end` — byte offsets (0-based, end exclusive)

```rust
let err = Error::parse("Unexpected token", 3, 15, 42, 44);
assert_eq!(err.line(), Some(3));
assert_eq!(err.column(), Some(15));
assert_eq!(err.start(), Some(42));
```

### 8.2 Validation Errors

```rust
pub struct ValidationError {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub severity: ValidationSeverity,  // Error or Warning
    pub code: String,                    // e.g., "E001", "E200"
    pub start: Option<usize>,
    pub end: Option<usize>,
}

pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
}
```

### 8.3 Guard Rail Errors

Format operations have configurable guard limits that return structured errors:

- `E_GUARD_INPUT_TOO_LARGE` — input exceeds `max_input_bytes`
- `E_GUARD_TOKEN_BUDGET_EXCEEDED` — token count exceeds `max_tokens`
- `E_GUARD_AST_BUDGET_EXCEEDED` — AST node count exceeds `max_ast_nodes`
- `E_GUARD_SET_OP_CHAIN_EXCEEDED` — UNION/INTERSECT/EXCEPT chain exceeds `max_set_op_chain`

---

## 9. AST Traversal & Analysis

### 9.1 Traversal

```rust
use polyglot_sql::{parse, DialectType};
use polyglot_sql::traversal::*;

let ast = parse("SELECT a, b FROM t WHERE x > 1", DialectType::Generic).unwrap();
let columns = get_columns(&ast[0]);  // ["a", "b", "x"]
let tables = get_tables(&ast[0]);    // ["t"]
```

Available predicates (70+):
- `is_select`, `is_insert`, `is_update`, `is_delete`, `is_ddl`
- `is_join`, `is_where`, `is_group_by`, `is_order_by`, `is_limit`
- `is_function`, `is_aggregate`, `is_subquery`, `is_cte`
- `is_comparison`, `is_logical`, `is_arithmetic`
- `contains_subquery`, `contains_aggregate`, `contains_window_function`

Iterators: `DfsIter`, `BfsIter` for depth-first and breadth-first traversal.

### 9.2 AST Transforms

```rust
use polyglot_sql::ast_transforms::*;

// Rename tables
let renamed = rename_tables(expr, &[("old_name", "new_name")]);

// Add WHERE condition
let filtered = add_where(expr, col("active").eq(lit(true)));

// Remove LIMIT/OFFSET
let unlimited = remove_limit_offset(expr);
```

### 9.3 AST Diff

```rust
use polyglot_sql::diff::{diff, diff_with_config, DiffConfig};

let edits = diff(&source_expr, &target_expr, true);
for edit in &edits {
    if edit.is_change() {
        println!("{:?}", edit);
    }
}
```

Uses the ChangeDistiller algorithm with Dice coefficient matching for structural comparison.

### 9.4 Logical Planner

```rust
use polyglot_sql::planner::Plan;

let plan = Plan::from_expression(&expr);
// plan.root is a Step DAG
// plan.leaves() returns leaf steps
// plan.dag() returns the dependency graph
```

Step kinds: Scan, Filter, Project, Aggregate, Join, Sort, Limit, etc.

---

## 10. Optimizer Modules

The optimizer is available behind the `semantic` feature flag:

| Module | Purpose |
|---|---|
| `qualify_columns.rs` | Resolve unqualified column references to table.column |
| `qualify_tables.rs` | Expand table names with schema/catalog |
| `annotate_types.rs` | Infer and annotate expression types |
| `pushdown_predicates.rs` | Push WHERE conditions into JOINs |
| `pushdown_projections.rs` | Reduce columns to only what's needed |
| `eliminate_joins.rs` | Remove unnecessary JOINs |
| `eliminate_ctes.rs` | Inline single-use CTEs |
| `simplify.rs` | Simplify boolean expressions, constant folding |
| `normalize.rs` | Expression normalization |
| `canonicalize.rs` | Query canonicalization |
| `subquery.rs` | Subquery analysis |

---

## 11. Async Support

**Polyglot does not use async I/O** — it is a pure computational library. All operations are synchronous and CPU-bound:

- `parse()` — synchronous
- `generate()` — synchronous
- `transpile()` — synchronous
- `validate()` — synchronous
- `format()` — synchronous

This is by design: Polyglot operates on SQL strings in memory, with no network or filesystem I/O. For use in async contexts (Tokio, async-std), callers should use `tokio::task::spawn_blocking()` or similar to offload CPU-heavy parsing/transpilation to a blocking thread pool.

---

## 12. Feature Flags

| Flag | Description | Default |
|---|---|---|
| `all-dialects` | Enable all 32 dialect parsers | ✅ |
| `generate` | SQL generation from AST | ✅ |
| `transpile` | Cross-dialect transpilation (implies `generate`) | ✅ |
| `builder` | Fluent query builder API (implies `generate`) | ✅ |
| `ast-tools` | AST inspection & transform utilities | ✅ |
| `semantic` | Schema, resolver, lineage, optimizer, validation | ✅ |
| `openlineage` | OpenLineage payload generation (implies `semantic`) | ✅ |
| `diff` | AST diff support (implies `generate`) | ✅ |
| `planner` | Logical planning helpers | ✅ |
| `time` | Time-format conversion helpers | ✅ |
| `stacker` | Stack-growth protection for native builds | ✅ |
| `bindings` | TypeScript type generation via `ts-rs` | ❌ |
| `dialect-postgresql` | PostgreSQL dialect only | — |
| `dialect-mysql` | MySQL dialect only | — |
| ... (one per dialect) | Individual dialect selector | — |
| `function-catalog-clickhouse` | ClickHouse function catalog | ❌ |
| `function-catalog-duckdb` | DuckDB function catalog | ❌ |
| `function-catalog-all-dialects` | All function catalogs | ❌ |

Minimal WASM build (for constrained targets):
```toml
polyglot-sql = { version = "0.4", default-features = false, features = ["generate", "transpile", "dialect-postgresql", "dialect-mysql"] }
```

---

## References

- Source code examined: `/workspace/polyglot/crates/polyglot-sql/src/` (~241K LOC)
- Architecture documentation: `/workspace/polyglot/docs/sqlglot-architecture.md`
- Benchmark results: `/workspace/polyglot/docs/benchmark.md`
- README: `/workspace/polyglot/README.md`, `/workspace/polyglot/crates/polyglot-sql/README.md`
- CHANGELOG: `/workspace/polyglot/CHANGELOG.md`