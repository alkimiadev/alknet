# Metatensor: Schema-Driven Binary Tensor Format

**Status:** Research / concept design. The schema layer is proven (TypeBox + jsonschema interop confirmed by API inspection); the offset computation and mmap layer are designed but not yet implemented.
**Date:** 2026-06-20
**Scope:** A schema-driven binary tensor format extending safetensors, using TypeBox (JS) / jsonschema (Rust) as the layout specification language. Supports flat, struct, and blob tensor kinds for fixed-size, record-structured, and variable-length data. Memory-mappable, QUIC-streamable, and authorable via ujsx.

---

## Executive Summary

Metatensor is a binary data format where a JSON Schema (authored in TypeBox, validated by either TypeBox in JS or the `jsonschema` crate in Rust) describes the layout of binary data, and the binary data follows at schema-computed offsets. It extends the safetensor format — which is fixed-schema (every tensor is `{dtype, shape, data_offsets}`) — to arbitrary schemas, enabling struct tensors (records with typed fields), blob tensors (variable-length data via indirection), and nested layouts.

The format is: `header_length (8 bytes) + JSON header (schema + offsets) + raw binary data (row-major, at offsets the header describes)`.

The key insight: **the schema is the format.** A TypeBox schema like `Type.Object({conv1_weight: TensorRef, conv1_bias: TensorRef, ...})` is both:
- The validation spec (TypeBox's `Value.Check` in JS, jsonschema's `validator.is_valid` in Rust)
- The layout spec (field offsets computed from the schema's type sizes, the same way typebox-rs computed struct layouts)

No separate format definition, no separate parser, no separate validator. One schema, three uses: validate, compute offsets, access data.

---

## The Schema Layer: TypeBox ↔ jsonschema

### Why this works

TypeBox modules render to standard JSON Schema under `$defs` (the `Type.Module` wrapper is just `$defs`). A TypeBox schema like:

```typescript
const TensorRef = Type.Object({
  dtype: Type.Union([Type.Literal("F32"), Type.Literal("I16"), ...]),
  shape: Type.Array(Type.Number()),
  data_offsets: Type.Tuple([Type.Number(), Type.Number()])
});
```

serialized to JSON is:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "dtype": { "type": "string", "enum": ["F32", "I16", ...] },
    "shape": { "type": "array", "items": { "type": "number" } },
    "data_offsets": { "type": "array", "minItems": 2, "maxItems": 2, ... }
  },
  "required": ["dtype", "shape", "data_offsets"]
}
```

That JSON feeds directly into `jsonschema::validator_for(&serde_json::from_str(...))` on the Rust side. Zero translation. The same schema validates in both ecosystems.

### Custom type kinds (typedef.ts port)

The `typedef.ts` file at `/workspace/@alkdev/typebox/example/typedef/typedef.ts` (619 lines) defines custom TypeBox schema kinds that carry *binary layout* semantics:

- `TFloat32` — `[Types.Kind]: 'TypeDef:Float32'`, `static: number`, 4-byte float
- `TInt32` — `[Types.Kind]: 'TypeDef:Int32'`, `static: number`, 4-byte int
- `TStruct` — `[Types.Kind]: 'TypeDef:Struct'`, fields with byte offsets, record layout
- `TUnion` — tagged union of structs (discriminator + mapping)
- `TEnum` — string enum

These are registered in TypeBox via `TypeRegistry.Set<TFloat32>('TypeDef:Float32', (schema, value) => ValueCheck.Check(schema, value))` — a custom validator that runs when `Value.Check` encounters the `TypeDef:Float32` kind.

The Rust `jsonschema` crate supports the same pattern via custom keywords:

```rust
use jsonschema::{Keyword, ValidationError};
use serde_json::{Map, Value};

struct Float32Validator;

impl Keyword for Float32Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        // Validate that the instance is a number representable as f32
        match instance {
            Value::Number(n) if n.as_f64().is_some() => Ok(()),
            _ => Err(ValidationError::custom("expected f32")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.is_number()
    }
}

fn float32_factory(
    _parent: &Map<String, Value>,
    _value: &Value,
    _path: Location,
) -> Result<Box<dyn Keyword>, ValidationError<'static>> {
    Ok(Box::new(Float32Validator))
}

let validator = jsonschema::options()
    .with_keyword("TypeDef:Float32", float32_factory)
    .with_keyword("TypeDef:Int32", int32_factory)
    .with_keyword("TypeDef:Struct", struct_factory)
    .build(&schema)?;
```

Every `TypeRegistry.Set<...>('TypeDef:...', validator)` in `typedef.ts` maps to a `with_keyword("TypeDef:...", factory)` in Rust. Same semantics, different language, same JSON Schema wire format.

The `jsonschema` crate also supports custom format validators via `with_format("f32", |v| ...)`, for the binary-type-specific format annotations. And compiled validators are reusable (`validator_for(&schema)?` then call `validator.is_valid(&instance)` repeatedly) — same performance profile as TypeBox's `Value.Check`.

### What this eliminates

**typebox-rs becomes unnecessary as a schema engine.** The `/workspace/@alkimiadev/typebox-rs/` crate (builder, schema, registry, validate, value) is replaced by:
- `jsonschema` crate — validation, custom keywords, custom formats, compiled validators
- A small offset-computation module — walks a schema, computes byte offsets for each field based on type sizes (the one piece of real logic beyond jsonschema)
- Custom keyword implementations — the `TypeDef:Float32` / `TypeDef:Int32` / `TypeDef:Struct` validators (small functions, ~10 lines each)

No hand-rolled schema type system, no builder, no registry. The codebase drops from "a port of TypeBox" to "jsonschema + an offset map + ~50 lines of custom keyword implementations."

---

## The Three Tensor Kinds

### Flat tensor (fixed-size, homogeneous)

The safetensor format as it exists today. A single dtype, a shape, and a contiguous byte range.

```typescript
const FlatTensor = Type.Object({
  dtype: Dtypes,                    // "F32", "I16", etc.
  shape: Type.Array(Type.Number()),  // [20, 1, 5, 5]
  data_offsets: Type.Tuple([Type.Number(), Type.Number()])  // [start, end] byte offsets
});
```

Offsets: `dtype_size(dtype) * product(shape)`. Direct mmap — one region, typed view, done. This is the base case.

### Struct tensor (fixed-size, heterogeneous)

A record of typed fields, each a flat tensor or another struct. One binary region, multiple typed views.

```typescript
const Layer = Type.Object({
  weight: TensorRef,   // {dtype: "F32", shape: [20, 1, 5, 5], data_offsets: [0, 2000]}
  bias: TensorRef,     // {dtype: "F32", shape: [20], data_offsets: [2000, 2080]}
  running_mean: TensorRef,  // {dtype: "F32", shape: [20], data_offsets: [2080, 2160]}
  running_var: TensorRef,   // {dtype: "F32", shape: [20], data_offsets: [2160, 2240]}
});

const Model = Type.Object({
  conv1: Layer,
  conv2: Layer,
}, { additionalProperties: false });
```

The schema describes the structure; the offsets in each `TensorRef` point into the same binary region. Loading a model = mmap the file, construct typed views per the schema's field offsets. No copy, no deserialization — the schema is the access pattern.

This is "a row of records" where each row is a struct and the whole file is an array of structs. For a model file, the whole file is one struct; for a dataset, it's an array of structs.

### Blob tensor (variable-length)

The hard case: strings, ragged arrays, arbitrary blobs. Solved via indirection — a struct tensor holds metadata + offsets, a flat tensor holds the actual variable-length bytes.

```typescript
const BlobEntry = Type.Object({
  offset: Type.Number(),   // byte offset into the blob flat tensor
  length: Type.Number(),   // byte length of this blob
});

const TextDataset = Type.Object({
  // The index: one BlobEntry per string, stored as a struct tensor
  entries: Type.Array(BlobEntry),
  // The data: raw bytes, stored as a flat tensor (dtype: "U8")
  blob_data: Type.Object({
    dtype: Type.Literal("U8"),
    shape: Type.Array(Type.Number()),  // [total_blob_bytes]
    data_offsets: Type.Tuple([Type.Number(), Type.Number()])
  })
});
```

To read string `i`: look up `entries[i]` → get `(offset, length)` → slice `blob_data[offset..offset+length]` → decode. Two mmaps (index + data), O(1) random access. Long strings, ragged arrays, JSON blobs, images — all handled by the same indirection pattern.

The blob tensor is a combination of a struct tensor (the index) and a flat tensor (the blob store). The struct tensor's fields are metadata + offsets into the flat tensor. This is the same pattern as Apache Arrow's variable-length types (strings are stored as an offsets array + a byte buffer), but described in TypeBox schema instead of Arrow's IPC format.

---

## Offset Computation

The one piece of real logic beyond jsonschema. Given a schema, compute the byte offset and size of each field. This is the same computation the old typebox-rs implementation did for struct layouts.

```rust
struct OffsetMap {
    fields: Vec<(String, ByteRange)>,  // field name → byte range
    total_size: usize,
}

struct ByteRange {
    start: usize,
    end: usize,
}

fn compute_offsets(schema: &serde_json::Value) -> OffsetMap {
    // Walk the schema:
    // - TypeDef:Float32 → 4 bytes
    // - TypeDef:Int32 → 4 bytes
    // - TypeDef:Array of T → element_size * len
    // - TypeDef:Struct → recurse, sum field sizes, align
    // - TensorRef → use data_offsets from the header
    // - Array of struct → element_size * count
    // ...
}
```

The offset computation is schema-driven: the schema's type kinds determine byte sizes, the struct's field order determines offsets, alignment rules (from the schema's annotations or defaults) determine padding. The output is an `OffsetMap` — a flat table of `(field_path, byte_range)` pairs.

For flat tensors, the offset is already in the `data_offsets` field of the `TensorRef` — no computation needed. For struct tensors, the offsets are computed from the field types. For blob tensors, the offsets are stored *in* the struct tensor's fields (the `offset` and `length` of each `BlobEntry`), pointing into the flat blob tensor.

---

## Memory Mapping

The payoff of schema-driven offsets: direct mmap with typed views, no deserialization.

```rust
use memmap2::Mmap;

struct MetatensorFile {
    header: serde_json::Value,     // parsed JSON header (schema + tensor refs)
    schema: serde_json::Value,     // the TypeBox/jsonschema schema
    offset_map: OffsetMap,         // computed from schema
    data: Mmap,                     // mmap'd binary region
}

impl MetatensorFile {
    fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        // Read 8-byte header length prefix
        let header_len = u64::from_le_bytes(mmap[0..8].try_into().unwrap()) as usize;

        // Parse JSON header (schema + tensor refs with offsets)
        let header: serde_json::Value = serde_json::from_slice(&mmap[8..8+header_len])?;

        // The data region starts after the header
        let data_start = 8 + header_len;

        // Compute offsets from schema
        let offset_map = compute_offsets(&header);

        Ok(Self { header, schema, offset_map, data: mmap })
    }

    fn tensor(&self, field_path: &str) -> &[u8] {
        let range = self.offset_map.get(field_path).unwrap();
        &self.data[range.start..range.end]
    }

    fn tensor_as_f32(&self, field_path: &str) -> &[f32] {
        let bytes = self.tensor(field_path);
        // Safe because the schema validated the dtype as F32
        unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, bytes.len() / 4) }
    }
}
```

No copy. No deserialization. The schema is the access pattern. For a 10GB model file, opening is O(header size) — parse the JSON header, compute offsets, mmap the data. Accessing any tensor is O(1) — the OS page-faults the relevant pages on demand.

---

## QUIC Stream Mapping

The format maps naturally to QUIC streams (and therefore to alknet's BiStream):

- **Header stream** — the JSON header (schema + tensor refs with offsets). Small, sent first. The receiver knows the entire layout before any data arrives.
- **Per-tensor data streams** — each tensor's byte range is a separate QUIC stream. The receiver can request specific tensors on demand (lazy loading) or receive all in parallel. Stream priority can be set per-tensor (load the first layer's weights before the last layer's).

```
Stream 0: header (JSON: schema + offset table)
Stream 1: conv1.weight bytes [0..2000]
Stream 2: conv1.bias bytes [2000..2080]
Stream 3: conv2.weight bytes [2080..4080]
Stream 4: conv2.bias bytes [4080..4160]
...
```

This is the "maps naturally to QUIC streams" property — the offset table in the header tells the receiver which stream has which tensor's bytes. A model can be partially loaded (just the layers needed for inference), streamed incrementally (start computing on layer 1 while layer 2 is still transferring), or broadcast to multiple peers (each peer subscribes to the streams it needs).

Over irpc/alknet-call, this becomes: `load_model` is an operation whose output is a struct of `BufferId`s; the operation's implementation opens QUIC streams per the offset table and writes each tensor's bytes into a wgpu buffer. The caller gets back tensor handles and starts computing.

---

## ujsx Authoring

The schema is a TypeBox module, and TypeBox modules are ujsx-authorable via the `TsToModule` codegen (`/workspace/research/typebox_research/codegen/ts-to-module.ts`) and the ujsx-as-AST pattern (`/workspace/research/typebox_research/ujsx/*.gen.ts`).

Just as `<Sequential>` and `<Operation>` are ujsx components for compute graphs, `<Tensor>` and `<Struct>` and `<Field>` are ujsx components for data layouts:

```tsx
import { h } from "@alkdev/ujsx";
import { Tensor, Struct, Field, ArrayOf } from "./layout-components";

const ModelLayout = (
  <Struct name="ConvNet">
    <Field name="conv1">
      <Struct>
        <Field name="weight"><Tensor dtype="f32" shape={[20, 1, 5, 5]} /></Field>
        <Field name="bias"><Tensor dtype="f32" shape={[20]} /></Field>
        <Field name="running_mean"><Tensor dtype="f32" shape={[20]} /></Field>
        <Field name="running_var"><Tensor dtype="f32" shape={[20]} /></Field>
      </Struct>
    </Field>
    <Field name="conv2">
      <Struct>
        <Field name="weight"><Tensor dtype="f32" shape={[20, 20, 5, 5]} /></Field>
        <Field name="bias"><Tensor dtype="f32" shape={[20]} /></Field>
      </Struct>
    </Field>
  </Struct>
);

// render(ModelLayout) against a LayoutHostConfig → produces a TypeBox schema
// + computed offsets → can serialize/deserialize/mmap/stream the model
```

The `LayoutHostConfig`'s `createInstance("Struct", props)` builds a `Type.Object(...)`; `createInstance("Tensor", {dtype, shape})` builds a `TensorRef`; `appendChild` nests fields into the struct. The output is a TypeBox schema + offset table — the same data structure metatensor uses, authored declaratively as JSX.

The reconciler diffs layout changes the same way it diffs UI trees: add a layer (new `<Field>` child), change a shape (changed prop on `<Tensor>`), remove a parameter (removed `<Field>`). The diff is a schema delta, which is a model delta, which is a version bump. Model versioning is tree diffing.

### The category-theory foundation

The ujsx research docs mention the category-theory foundations of ujsx as a universal typed-tree IR. The application to binary layouts is the same foundation:

- A ujsx element is a typed tree node. `<Tensor dtype="f32" shape={[20]} />` is a node of type `Tensor` with props `{dtype, shape}`.
- A component is a typed tree constructor. `<Struct>` constructs a `Type.Object(...)`.
- The reconciler is a typed tree diff. Changing `shape={[20, 1, 5, 5]}` to `shape={[40, 1, 5, 5]}` is a prop diff that produces a schema delta.
- The `HostConfig` is the interpreter. `LayoutHostConfig` interprets the tree as a schema + offset table; `GraphologyHostConfig` interprets it as a DAG; `ThreeHostConfig` interprets it as a three.js scene graph.

`<Tensor>` is no stranger than `<div>`. Both are typed tree nodes. The `HostConfig` decides what they *mean*. This is why ujsx works as a universal IR — the tree structure is generic, the interpretation is pluggable.

---

## Relationship to Existing Formats

| Format | Schema | Binary layout | Variable-length | Memory-mappable | Stream-friendly | Safe (no code exec) |
|--------|--------|--------------|-----------------|-----------------|-----------------|-------------------|
| PyTorch `.pt` | pickle (unsafe) | pickle | yes | no | no | **no** (arbitrary code) |
| safetensors | fixed (TensorRef) | flat only | no | yes | partial | yes |
| Apache Arrow IPC | Arrow schema | columnar | yes (offsets) | yes | yes | yes |
| metatensor | TypeBox/jsonschema (arbitrary) | flat + struct + blob | yes (blob indirection) | yes | yes (QUIC per-tensor) | yes |

metatensor sits between safetensors (simpler, fixed schema) and Arrow (more complex, columnar). The distinguishing property: the schema is TypeBox/jsonschema, so it's the same schema system used everywhere else in the alkdev ecosystem — operations, ujsx, flowgraph, the call protocol. Arrow has its own schema format; metatensor uses yours.

---

## What This Enables

### Model serialization

A model is a struct tensor. Save = serialize schema (TypeBox → JSON) + write tensor bytes at computed offsets. Load = parse header, compute offsets, mmap. No pickle, no code execution, safe to load untrusted models.

### Dataset storage

A dataset is an array of struct tensors. Each record is a struct (fields: input tensor, label tensor, metadata blob). The blob tensor handles variable-length text fields. Memory-map the whole dataset, access records by index, stream over QUIC per-record on demand.

### Network transport

The header (schema + offsets) is one small JSON message. The tensor bytes are QUIC streams. A receiver can:
- Load the schema, validate it, know the full layout before any data arrives
- Request specific tensors (lazy load just the layers needed)
- Receive in parallel (multiple QUIC streams concurrently)
- Start computing on early tensors while later ones are still transferring

Over irpc: `load_model` is an operation that opens the file, mmaps it, returns a struct of `BufferId`s (one per tensor, written to wgpu buffers from the mmap'd bytes). Or: `stream_model` is an operation that sends the header then opens QUIC streams per tensor, writing to the receiver's wgpu buffers as each stream arrives.

### Schema evolution

The schema is a TypeBox module. TypeBox has `Value.Diff`, `Value.Migrate`, `Value.Convert` — schema evolution is built in. Adding a field to a model is a schema delta; old files still load (the new field is optional or has a default). Changing a field type is a migration with a converter function. Model versioning is schema versioning, which is tree diffing, which is what the ujsx reconciler already does.

---

## Open Questions

### 1. Alignment and padding rules

Different platforms have different alignment requirements for GPU upload (wgpu may want 256-byte alignment for some buffer types). The offset computation needs to respect alignment, which may be schema-annotated (`Type.Number({ align: 256 })`) or globally configured. Need to decide: is alignment a schema property, a format property, or a runtime property?

**Recommendation:** schema-annotated with a sensible default (4-byte for floats, 16-byte for structs). The offset computation respects the annotation; wgpu buffer creation rounds up as needed.

### 2. Endianness

safetensors is little-endian. metatensor should be too (wgpu buffers are little-endian on all supported backends). But the schema should probably declare it (`{ endian: "little" }` in the header) for completeness. Cross-endian platforms (none currently supported by wgpu) would need a byte-swap on load.

### 3. Compression

safetensors is uncompressed. For large models, compression matters. Options: compress the whole data region (zstd), compress per-tensor (each QUIC stream compressed independently), or leave compression to the transport layer (QUIC has built-in compression via HTTP/3 header compression; irpc could add a compression layer). Need to decide where compression lives.

**Recommendation:** transport-layer compression (QUIC/irpc handles it), format is uncompressed on disk. Keeps the format simple; compression is a deployment concern.

### 4. The blob tensor's two-region layout

The blob tensor requires two binary regions (index struct + data flat tensor). This means the metatensor format needs to support multiple data regions, not one contiguous blob. Options: the header lists multiple data regions with their own offset ranges, or the blob tensor's `data_offsets` are absolute (pointing into the single data region, with the index at the start and the blob data after). Need to decide the region model.

**Recommendation:** single data region, absolute offsets. The index struct lives at the start of the data region; the blob data follows. All offsets are absolute from the start of the data region. Simple, one mmap, the offset map handles indirection.

### 5. Integration with wgpu buffers

Loading a tensor from mmap into a wgpu buffer: is it a copy (mmap → staging buffer → GPU buffer) or a direct map (if wgpu supports host-visible buffers)? Depends on the GPU backend — discrete GPUs need a copy; integrated/llvmpipe might map directly. The `load_model` operation needs to handle both paths.

---

## References

- **TypeBox (JS schema):** `/workspace/@alkdev/typebox/` — `src/type/` (builders), `src/value/` (Value.Check, Value.Diff, Value.Clone, Value.Equal — verified on QuickJS-NG by POC 2)
- **typedef.ts (binary layout types):** `/workspace/@alkdev/typebox/example/typedef/typedef.ts` (619 lines) — `TFloat32`, `TInt32`, `TStruct`, `TUnion`, `TEnum` with `TypeRegistry.Set<...>` custom validators
- **jsonschema (Rust validation):** `/workspace/jsonschema` — `validator_for(&schema)?`, `with_keyword("TypeDef:...", factory)` for custom kinds, `with_format("...", validator)` for custom formats, Draft 2020-12/2019-09/7/6/4 support
- **typebox-rs (to be replaced by jsonschema + offset map):** `/workspace/@alkimiadev/typebox-rs/` — `src/codegen/` (handlebars codegen: RustGenerator, TypeScriptGenerator — WgslGenerator would be the third backend here)
- **metatensor concept:** `/workspace/research/typebox_research/metatensor/basic.ts` (78 lines) — `Dtypes`, `TensorRef`, `TensorMap` TypeBox schemas, safetensor header parsing, `Value.Check(TensorMap, header)` validation
- **ujsx as AST:** `/workspace/research/typebox_research/ujsx/` — `ujsx.ts` (UJSX TypeBox module), `unist.gen.ts` (unist AST → TypeBox module), `mdast.gen.ts` (mdast AST → TypeBox module), `jpath.gen.ts` (JSONPath AST → TypeBox module)
- **TsToModule codegen:** `/workspace/research/typebox_research/codegen/ts-to-module.ts` — generates TypeBox modules from TypeScript type definitions via the TypeScript compiler API
- **safetensor format reference:** `/workspace/research/typebox_research/metatensor/basic.ts` — 8-byte LE header length + JSON header + raw bytes; `TensorRef = {dtype, shape, data_offsets}`
- **ujsx reconciler (verified on quickjs):** `/workspace/@alkdev/ujsx/src/host/{reconcile,config,fiber}.ts` — fiber-based reconciler, the typed-tree diff engine
- **flowgraph (compute graph layer, uses ujsx):** `/workspace/@alkdev/flowgraph/` — `<Operation>`, `<Sequential>`, `<Parallel>`, `<Conditional>`, `<Map>` components; `GraphologyHostConfig` + `ReactiveHostConfig`
- **alknet-tensor architecture (parent doc):** `/workspace/@alkdev/alknet/docs/research/alknet-tensor/architecture-summary.md` — the tensor compute architecture this format serves