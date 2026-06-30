# alknet-compute: Tensor Compute Engine (Research Summary)

**Status:** Early research — architecture direction established, no POCs yet. Derived from analyzing `webgpu-torch` as a reference design. This doc was previously titled `alknet-tensor/architecture-summary.md`; the crate-decomposition session on 2026-06-30 split the original `alknet-tensor` concept into two crates: `alknet-tensor` (the pure-format metatensor binary layout, now at `docs/research/alknet-tensor/metatensor-format.md`) and `alknet-compute` (the wgpu compute engine — this doc). The compute engine builds on `alknet-runtime` (the JS+wgpu substrate, `docs/research/alknet-runtime/summary.md`) and `alknet-tensor` (the format).
**Date:** 2026-06-20 (original), 2026-06-30 (reframed for crate split)
**Scope:** Captures the architectural direction for the wgpu compute engine: buffer management, kernel codegen, autograd-via-flowgraph, distributed training over irpc. Uses `alknet-runtime` for the JS isolate, wgpu device, and ops bridge into alknet-call's registry; uses `alknet-tensor` for the binary model format. Documents what `webgpu-torch` established as a reference, how the architecture differs from a straight port, and what unknowns remain.

---

## Executive Summary

`alknet-compute` is a PyTorch-shaped tensor computation layer built on the `alknet-runtime` substrate (Rust + wgpu + QuickJS via rquickjs) and `alknet-tensor` (the binary format). It owns the tensor-shaped abstractions: `BufferId`-handle buffer manager, the `OpSpec`/`KernelSpec` op table, the `ShaderGenerator` codegen pipeline, the ~5 high-level Rust ops, autograd-via-flowgraph, and distributed training. It does not own the JS isolate, the wgpu device, or the operations-protocol bridge — those live in `alknet-runtime`. It does not own the binary format — that lives in `alknet-tensor`.

It is derived from the design of `webgpu-torch` (`/workspace/webgpu-torch`) — a pure-JS tensor + autograd library that runs entirely on the WebGPU compute pipeline — but is not a port of its code. webgpu-torch is the *reference design*; alknet-compute is the *production architecture*.

The substrate this builds on is verified by the alknet-desktop POCs and captured in `docs/research/alknet-runtime/summary.md`:

1. **wgpu on llvmpipe (software Vulkan) is genuinely useful compute with no physical GPU** — WGSL compiles to optimized SIMD, beats JS for any non-trivial workload, and the same WGSL runs at full GPU speed when a GPU is present. Tensor compute is testable on this OVH box right now, deployable to vast.ai GPU instances for production. The runtime acquires the wgpu device; alknet-compute uses it.
2. **QuickJS-NG runs the operations protocol (`@alkdev/operations` registry, call, envelopes, ACL, `buildCallHandler`)** — verified by POC-2. Every tensor op can be an `OperationSpec` on the registry, network-callable over irpc, same as any other operation. The runtime owns the ops bridge; alknet-compute registers its ops on the runtime's registry.
3. **`typebox-rs` has the handlebars codegen pattern** (`/workspace/@alkimiadev/typebox-rs/src/codegen/`) — `RustGenerator` and `TypeScriptGenerator` render typed schemas to target languages; a `ShaderGenerator` trait with a `WgslGenerator` impl is the same shape, rendering `KernelSpec` → shader strings. The trait is parameterized by shading language (WGSL first, SPIR-V / GLSL / naga-IR later) per wgpu's multi-input-language support.

This solves several downstream problems that weren't the original target (see §Downstream Problems Solved).

---

## Reference Design: webgpu-torch

**Location:** `/workspace/webgpu-torch` (v0.4.0, npm-published, zero runtime deps except `@webgpu/types`, `@xtuc/long`, `cross-fetch`)
**Homepage:** https://praeclarum.org/webgpu-torch

### What it is

A PyTorch-like ML library that implements tensors, autograd, an `nn` module hierarchy, optimizers, and ONNX import/export — all in TypeScript, all running on WebGPU compute pipelines. No CUDA, no native bindings, no browser required (works in Deno with `--unstable-webgpu`).

### The three-stage pipeline

webgpu-torch's op system is structured in three clean stages, each of which is relevant to the alknet-tensor architecture:

**Stage 1 — `OpSpec` (declarative op description).** (`src/op_spec.ts:8-27`, `src/op_table.ts` — 452 lines, ~100 ops)

```typescript
type OpSpec = {
  name: string;
  nnName?: string;       // torch.nn name (e.g. "ReLU")
  torchName?: string;    // torch.* name
  nnOp?: boolean;        // is this an nn module?
  type: "unary" | "binary" | "reduction";
  forward: ExprCode;     // e.g. "output = abs(input)"
  backward?: ExprCode;   // e.g. "inputGrad = input == 0 ? 0 : ..."
  alpha?: boolean;       // binary ops with alpha scalar
  // reduction-specific:
  init?: ExprCode;       // e.g. "0" for sum
  combineOp?: "+" | "*" | "&&" | "||";
  reduce?: ExprCode;
};
```

The entire op table is declarative data — ~100 ops (abs, acos, add, matmul, conv2d, layer_norm, etc.) described as forward/backward expressions. No imperative dispatch code, no buffer management, no GPU calls. This is the schema layer.

**Stage 2 — `opgen.ts` (op spec → kernel specs).** (`src/opgen.ts`, 728 lines)

Transforms each `OpSpec` into one or more `KernelSpec` entries — one per dtype combination and gradient direction. A binary op like `add` produces 6+ kernel specs (forward for each dtype pair, plus backward variants). A `KernelSpec` (`src/kernel.ts:34-45`) is a complete compute-pass description:

```typescript
type KernelSpec = {
  name: string;
  parameters: KernelParamSpec[];      // scalar params (alpha, dims, etc.)
  inputs: KernelInputSpec[];           // storage buffer bindings
  outputs: KernelOutputSpec[];         // read_write storage buffer bindings
  workgroupSize: [ExprCode, ExprCode, ExprCode];
  workgroupCount: [ExprCode, ExprCode, ExprCode];
  workgroupVariables?: KernelInputSpec[];
  shader: string;                      // the WGSL body (without scaffolding)
};
```

This stage is pure computation — array manipulation and expression compilation (`ExprCode` → compiled shader fragment). No GPU calls, no side effects. It runs fine in JS but could also run in Rust.

**Stage 3 — `getKernelShaderCode` (kernel spec → final WGSL).** (`src/kernel.ts:299-375`, ~70 lines)

Turns a `KernelSpec` into a complete WGSL shader by string-concatenating:

- `struct ${name}Parameters { ... }` — parameter struct
- `@group(0) @binding(N) var<storage, read> input: ...` — input bindings
- `@group(0) @binding(N) var<storage, read_write> output: ...` — output bindings
- `@compute @workgroup_size(x, y, z)` — compute entry point header
- `@builtin(global_invocation_id) global_id: vec3u` — conditionally included if the shader references `global_id`
- The shader body from `spec.shader`

This is template rendering — loops over inputs/outputs/parameters, conditional `@builtin` inclusion. It is exactly what handlebars does, and exactly the pattern `typebox-rs` codegen already uses.

### The autograd system

`src/autograd.ts` (112 lines) — `GradientContext`, `AutoFunction`, backward dispatch. The autograd graph is pure bookkeeping: which op produced which tensor, what's the backward function, which tensors to save for backward. No heavy compute — just metadata wiring. `backward()` calls back into the kernel dispatch to run the backward shaders.

This stays in JS in alknet-tensor. It's the composition layer: users write `loss.backward()` and the graph traversal calls Rust-side backward kernels. The graph itself is lightweight (tensor handles + op references, no data).

### The nn module hierarchy

`src/nn_module.ts` (467 lines) — `Module` base class with `_children` tree, `Parameter` (tensor with `requiresGrad`), `StateDict` for serialization. `src/nn_basic.ts`, `nn_2d.ts`, `nn_norm.ts`, `nn_diffusers.ts`, `nn_applications.ts` implement Conv2d, BatchNorm, Linear, attention, etc.

This is composition structure — it builds the call graph, not the compute. Stays in JS.

### The optimizer

`src/optim.ts` (204 lines) — `Optimizer` base class, param groups, state tracking. Stays in JS (it's a loop over parameters calling Rust-side ops).

### The GPU API surface it uses

Small and entirely compute-oriented (no render passes, no swapchain, no textures-as-render-targets):

`createBuffer`, `createShaderModule`, `createComputePipeline`, `createBindGroup`, `beginComputePass`, `dispatchWorkgroups`, `copyBufferToBuffer`, `mapAsync`, `writeBuffer`.

~10 distinct GPU API calls, all on the compute side. This is the *easier* half of wgpu to expose from Rust — no surface management, no present loop, no window handles. Tensor compute is structurally simpler than the UI rendering case.

---

## The Architecture: JS as API, Rust as Execution

The key architectural decision: **JS holds handles, Rust owns memory and dispatch.** This is the PyTorch model (Python holds handles, C++/CUDA owns memory) applied to QuickJS + wgpu. Under the crate split, the JS isolate and wgpu device live in `alknet-runtime`; `alknet-compute` owns the tensor-shaped abstractions on top.

### What lives in alknet-runtime (the substrate)

- **The JS isolate** (rquickjs + QuickJS-NG, the 271-module shared core bundle)
- **The wgpu device** (acquired unconditionally; llvmpipe on CPU-only boxes, real GPU when present)
- **The operations-protocol bridge** into alknet-call's `OperationRegistry` — tensor ops registered here become `OperationSpec`s, network-callable via `CallClient`/`from_call` (ADR-017)
- **Primitive compute dispatch** — compile shader module, create buffer, dispatch compute pass, readback. `alknet-compute`'s high-level ops are built on these primitives.
- **Sandbox / privilege model** — `allowFetch`/`allowFs`/`envProxy` gates

### What lives in alknet-compute (this crate)

#### JS layer (thin API/composition, no tensor data, no GPU calls)

- **Tensor** = `{id: BufferId, shape: number[], dtype: string, requiresGrad: boolean, grad: Tensor | null}` — metadata only, the data is a Rust-owned `wgpu::Buffer`
- **Op table** — declarative `OpSpec` definitions (same schema as webgpu-torch's, possibly as TypeBox schemas for registry integration)
- **Autograd graph** — `GradientContext`, `AutoFunction`, backward bookkeeping. Pure metadata wiring.
- **nn module hierarchy** — `Module`, `Parameter`, `Sequential`, `Conv2d`, `Linear`, etc. Composition structure that builds the call graph.
- **Optimizer** — param groups, state, the `step()` loop. Calls Rust-side ops.
- **Custom kernel registration** — user writes a shader string, calls `register_kernel(name, shader, input_specs, output_specs)`. Rust compiles and caches.
- **Operations registry integration** — each tensor op is an `OperationSpec` (verified on quickjs by POC 2). Built-in ops register at init; user ops register dynamically. All network-callable over irpc.

#### Rust layer (memory, dispatch, codegen — the execution layer)

- **Buffer manager** — `HashMap<BufferId, wgpu::Buffer>` with manual lifetime management. Replaces webgpu-torch's `FinalizationRegistry`-driven JS buffer pool with Rust-native resource management. No GC interaction, no weak refs, deterministic destruction.
- **Kernel compiler** — `wgpu::ShaderModule` creation from shader strings (WGSL by default; SPIR-V / GLSL / naga-IR via wgpu's input-language features). Built-in kernels compiled at startup (or build time via handlebars codegen); custom kernels compiled on `register_kernel` call. Pipeline cache by shader hash.
- **Dispatch** — bind groups, compute pass encoding, `dispatchWorkgroups`, command submission. One Rust op per dispatch shape. Built on `alknet-runtime`'s primitive compute dispatch.
- **Shader codegen** — `ShaderGenerator` trait (handlebars-rs) renders `KernelSpec` → shader string. `WgslGenerator` is the first impl; `SpirvGenerator` / `GlslGenerator` / `NagaIrGenerator` are later backends per wgpu's multi-input-language support. Same pattern as `typebox-rs`'s `RustGenerator` / `TypeScriptGenerator`. Build-time codegen for built-in ops; runtime compilation for custom kernels.
- **Readback** — `copyBufferToBuffer` to a mapped read buffer, return `ArrayBuffer` to JS. The only data-crossing op (explicit, like PyTorch's `.cpu()` / `.numpy()`).

### What lives in alknet-tensor (the format crate, sibling not child)

- **Binary layout** — schema-driven offsets, flat/struct/blob tensor kinds, mmap via `memmap2`, QUIC per-tensor stream mapping
- **No JS or wgpu dependency** — a pure-Rust model server can use the format without `alknet-runtime`
- **Bridge to compute** — `alknet-compute` registers the `load_model`/`stream_model` ops that read a metatensor file into wgpu buffers; the format crate itself doesn't know about wgpu

### The Rust op surface (alknet-compute's high-level ops, built on runtime primitives)

Minimal — ~4-5 high-level ops, not ~20 low-level GPU API calls:

| Op | Signature | Purpose |
|----|-----------|---------|
| `create_tensor` | `(data: ArrayBuffer, shape: number[], dtype: string) → BufferId` | Allocate a storage buffer, write initial data |
| `dispatch_kernel` | `(name: string, inputs: BufferId[], params: object, workgroup_count: [u, v, w]) → BufferId[]` | Look up compiled kernel, bind inputs, dispatch compute pass, return output buffer IDs |
| `register_kernel` | `(name: string, shader: string, input_specs: KernelInputSpec[], output_specs: KernelOutputSpec[]) → void` | Compile custom shader (WGSL/SPIR-V/GLSL/naga-IR), cache by name |
| `read_tensor` | `(buffer_id: BufferId) → ArrayBuffer` | Copy buffer to mapped read buffer, return data to JS |
| `write_tensor` | `(buffer_id: BufferId, data: ArrayBuffer) → void` | Overwrite buffer contents from JS |

The data-crossing boundary is `read_tensor` / `write_tensor` only. A matmul on a 4096×4096 tensor is one `dispatch_kernel` call passing three `BufferId`s — the 64MB of floats never touch JS.

### The codegen pipeline

```
Build time:
  OpSpec[] (declarative, from op table)
    → opgen transform (opgen.ts logic, in Rust or JS)
    → KernelSpec[] (compute-pass descriptions)
    → ShaderGenerator::render(KernelSpec) → shader string (WGSL first)
    → wgpu pre-compiles each shader → ShaderModule (cached by name)

Runtime (built-in ops):
  JS calls dispatch_kernel("matmul", [a_id, b_id], params, count)
  → Rust looks up cached pipeline for "matmul"
  → binds buffers, dispatches, returns output BufferId

Runtime (custom kernels):
  JS calls register_kernel("my_op", shader_string, inputs, outputs)
  → Rust compiles shader via wgpu::ShaderModule (language per wgpu features)
  → caches pipeline by name
  → subsequent dispatch_kernel("my_op", ...) uses the cached pipeline
```

The `ShaderGenerator` trait (with `WgslGenerator` as the first impl) is the natural third backend in `typebox-rs`'s codegen module:

```
typebox-rs/src/codegen/
├── mod.rs          — pub use RustGenerator, TypeScriptGenerator, ShaderGenerator
├── rust.rs         — Schema → Rust structs (existing)
├── typescript.rs   — Schema → TS interfaces (existing)
└── shader.rs       — KernelSpec → shader string (new; WgslGenerator + later backends)
```

The WGSL template encodes the scaffolding from webgpu-torch's `getKernelShaderCode` (`kernel.ts:299-375`): struct declarations, `@group(0) @binding(N)` declarations, `@compute @workgroup_size` header, conditional `@builtin` inclusion. One handlebars template with `{{#each inputs}}`, `{{#each outputs}}`, `{{#if uses_global_id}}` blocks. The trait abstraction means a SPIR-V or GLSL template can be added later without changing `KernelSpec` or the opgen transform — only the final render step is language-specific.

---

## Downstream Problems Solved

This wasn't the original target, but the tensor architecture solves several planned problems as a side effect:

### 1. Distributed compute over irpc

Every tensor op is an `OperationSpec` on the registry (verified protocol-compatible on quickjs by POC 2). A `matmul` called locally dispatches on the local GPU. The same `matmul` called over irpc dispatches on a peer's GPU. This is the "vast.ai instance" deployment story with a concrete protocol backing it — no separate RPC layer needed, the operations registry *is* the RPC layer.

Distributed training follows: gradient ops, optimizer steps, and parameter sync are all operations, callable locally or remotely, with ACL enforcement on who can touch which model weights. Gradient sync across nodes is `read_tensor` + irpc `write_tensor` to the remote buffer.

### 2. LLM-authored model code (toolEnv pattern)

An agent emits JS that constructs an `nn.Sequential` and registers it as an operation, with `allowFetch: false` / `allowFs: false` sandboxing (the toolEnv privilege model from `/workspace/toolEnv/core/sandbox/`). The JS runs in a quickjs isolate, the compute runs in Rust/wgpu, the agent never touches the GPU directly. "MCP with scripting capabilities" extended to model authoring — an LLM composes a model architecture from declarative nn modules, the heavy ops execute on GPU.

### 3. Edge/embedded tensor compute

QuickJS-NG's 210 KiB footprint + wgpu's cross-platform backends (including llvmpipe software fallback) means tensor compute works where PyTorch can't fit — no Python runtime, no CUDA dependency, no large native binaries. The same JS model code runs on a server GPU (Vulkan/Metal/DX12), a laptop (same), or a headless box (llvmpipe, slower but functional).

### 4. The compositing problem from alknet-desktop

The alknet-desktop research doc flagged "compositing 3D + 2D onto one surface" as an open unknown. Tensor compute sidesteps it entirely — compute pipelines have no surface, no swapchain, no present loop. The compositing complexity is a *render* problem; tensor ops are pure compute. This makes alknet-compute structurally simpler than alknet-desktop despite being a "heavier" workload.

### 5. Cross-platform by construction, not configuration

wgpu's "one API, many backends" design means the same shaders and the same dispatch code run on Vulkan (Linux), Metal (macOS), DX12 (Windows), and llvmpipe (anywhere). No `#ifdef CUDA`, no "Linux is second-class", no platform-specific build matrix. The op table is shader strings (WGSL by default); the execution is wgpu; the platform is whatever wgpu supports. Currently: everything.

---

## Relationship to alknet-desktop (via alknet-runtime)

alknet-compute and alknet-desktop are sibling consumers of `alknet-runtime`. They don't depend on each other directly; both depend on the runtime for the JS isolate, wgpu device, and ops bridge. A desktop app that also does in-process ML depends on both (desktop → runtime, desktop → compute), sharing the one wgpu device the runtime acquires.

| | alknet-runtime (substrate) | alknet-desktop (sibling consumer) | alknet-compute (this crate) |
|---|---|---|---|
| **Owns** | JS isolate, wgpu device, ops bridge, shared JS core, sandbox, primitive compute dispatch | winit, surface/swapchain, three.js shims, Three/SDF HostConfigs, compositor, irpc-to-head | Buffer manager, op table, `ShaderGenerator`, tensor ops, autograd-via-flowgraph, `gradcheck`, distributed training |
| **wgpu usage** | Device acquisition + primitive compute dispatch | Render passes, surfaces, swapchain, compositing | Compute passes only — no surface, no swapchain |
| **GPU op surface** | Primitive: compile_shader/create_buffer/dispatch/readback | ~25-40 ops (browser globals for three.js + surface management) | ~4-5 ops (create/dispatch/register/read/write) layered on runtime primitives |
| **JS layer** | Shared core bundle (271 modules) | + three.js + Three/SDF HostConfigs | + flowgraph + reactive execution host + op table + autograd graph |
| **Complexity driver** | The extraction boundary (what's truly shared) | 3D+2D compositing, three.js shim surface | Autograd graph correctness, kernel codegen, distributed training |
| **Network model** | Ops bridge into alknet-call registry | Desktop worker dials head, renders UI (ADR-017) | Tensor ops on registry, distributed via `from_call` (ADR-017) |

The operations registry (owned by alknet-call, bridged by alknet-runtime) is the shared seam — both consumers register their ops on the same registry, and both become network-callable via `CallClient`/`from_call`.

---

## Open Unknowns

### 1. Where does the op table live — Rust or JS?

If built-in ops are Rust-side (specs compiled at build time via handlebars `ShaderGenerator`/`WgslGenerator`, kernels pre-registered), JS just calls `matmul(a, b)` and Rust looks up the compiled kernel. Fast, simple, fixed op surface.

If the op table stays JS-side (op specs as data in JS, sent to Rust at init to compile), it's more flexible — swap op implementations at runtime, let users inspect/override specs, let LLMs generate new ops. Adds a startup cost and more JS↔Rust traffic at init.

**Recommendation:** Rust-side for built-ins (build-time codegen, pre-compiled), JS-side `register_kernel` for custom/user-defined ops. Gets both perf and flexibility. The `OperationSpec` wrapper on the registry is what makes them network-callable regardless of where the kernel was compiled.

### 2. Does `opgen.ts`'s `ExprCode` parser/compiler port cleanly to Rust?

The `ExprCode` system (`src/expr.ts`) parses forward/backward expressions like `"output = abs(input)"` and compiles them to shader fragments. This is the one non-trivial JS piece in stage 2. If it ports to Rust (via `nom` or `pest` or hand-rolled), stage 2 moves entirely to Rust and the op table becomes pure data that never touches JS. If it doesn't port cleanly, stage 2 stays in JS and sends `KernelSpec` to Rust at init.

**Probeable:** read `src/expr.ts`, assess the parser complexity. If it's regex + string substitution (likely, given the WGSL target), the Rust port is mechanical. If it's a recursive-descent parser with non-trivial precedence handling, more work.

### 3. Autograd graph correctness

webgpu-torch's autograd (`src/autograd.ts`, 112 lines) is compact but subtle — `GradientContext`, `saveForBackward`, `needsInputGradient`, the backward dispatch. Porting the *design* to JS-on-quickjs is straightforward (it's pure bookkeeping), but verifying gradient correctness across the op table requires a test harness. PyTorch's `torch.autograd.gradcheck` (numerical gradient verification) is the reference approach — finite-difference against analytical gradients.

**Probeable:** implement `gradcheck` as an operation on the registry, run it against a subset of the op table (abs, add, matmul, conv2d) to verify the backward expressions are correct. This is a test problem, not an architecture problem.

### 4. Buffer management strategy

webgpu-torch uses a `FinalizationRegistry`-driven buffer pool in JS (`src/device_webgpu.ts:13-50`) — when a JS tensor is GC'd, the underlying `GPUBuffer` returns to the pool. Under alknet-tensor, Rust owns the buffers, so the pool is a Rust `HashMap` with explicit `drop_buffer(id)` or reference counting. The question is the lifecycle model: explicit `tensor.dispose()` (PyTorch-style, manual), RAII via Rust's `Drop` (automatic when the JS handle is GC'd and Rust is notified), or a pool with eviction.

**Recommendation:** explicit `dispose()` for now (simplest, matches PyTorch's `.detach()` / context manager pattern), with a Rust-side leak detector that warns if buffers aren't disposed. RAII-via-GC-notification is a later optimization.

### 5. Multi-GPU and multi-queue

wgpu supports multiple adapters and queues. For distributed training across GPUs on one machine (or across machines via irpc), the dispatch needs to target a specific queue/adapter. The `BufferId` likely needs to be `(AdapterId, BufferId)` or the dispatch op takes an optional `device` parameter. Not a blocker for v1 (single-GPU), but the op signatures should be designed to accept it.

### 6. typebox-rs simplification (serde + jsonschema)

You noted that typebox-rs should be rewritten to use serde + jsonschema instead of the hand-rolled schema system. This simplifies the schema layer and makes `KernelSpec` / `OpSpec` directly serde-serializable (for irpc transport, for config files, for LLM-generated op specs). The codegen layer (`handlebars-rs` + templates) stays; only the input schema type changes. This is a prerequisite for clean `KernelSpec` serialization over the wire.

---

## Recommended Next POCs

In priority order:

1. **WGSL codegen probe** — write the `WgslGenerator` handlebars template against `KernelSpec`, render all ~100 ops from `op_table.ts`, diff output against `getKernelShaderCode`'s output. If they match, the Rust codegen path is proven. Half-day exercise.

2. **`ExprCode` parser assessment** — read `src/expr.ts`, determine if the parser ports to Rust cleanly. If yes, stage 2 moves to Rust entirely. If no, stage 2 stays in JS and sends `KernelSpec` to Rust at init.

3. **End-to-end compute skeleton** — Rust crate that creates a wgpu device on llvmpipe, exposes `create_tensor` / `dispatch_kernel` / `read_tensor` to quickjs, and runs a hardcoded matmul. Proves the ~4-op Rust surface is sufficient and the buffer management works. One day.

4. **`gradcheck` test harness** — implement finite-difference gradient verification as an operation, run against a subset of the op table. Proves the autograd design is correct before porting the full graph. Half-day.

---

## Compute Graphs: flowgraph + ujsx as the Execution Layer

**Location:** `/workspace/@alkdev/flowgraph` (npm-published, uses ujsx)
**Relevance:** Replaces webgpu-torch's imperative autograd + nn module hierarchy with a declarative, reactive, graph-validated compute graph authoring and execution system. This is the CUDA-graphs-shaped layer, and it's already built.

### The insight

webgpu-torch's `nn.Module` hierarchy is an imperative call-graph: you write `forward(x)` that chains op calls, and autograd records the graph as a side effect. flowgraph inverts this — you write the graph declaratively as a ujsx tree, the graph is validated before execution, and reactive signals drive the execution. The ujsx tree *is* the compute graph, and the existing `@alkdev/flowgraph` library already implements this for the operations protocol that alknet-tensor uses.

### What flowgraph provides

flowgraph sits between `@alkdev/operations` (what can be called) and execution. It defines three graphs:

1. **Operation Graph** — static graph built from `OperationSpec`s at startup. Nodes are operations, edges are type-compatibility relationships. Enables cycle detection, topological ordering, validation.
2. **Call Graph** — dynamic graph built from call protocol events at runtime. Nodes are call invocations with status/timestamps, edges are parent-child. Enables abort cascading and observability.
3. **Workflow Template** — declarative ujsx tree defining a reusable workflow structure. A validated path through the operation graph, instantiated as a call graph at runtime.

**The graph is the specification. The template is the authoring surface. The call graph is the execution record.**

The workflow components (`/workspace/@alkdev/flowgraph/src/component/`):

- `<Operation name="tensor.matmul" input={...} />` — a single op call, like a kernel launch
- `<Sequential>` — ordered execution, outputs flow to inputs (CUDA stream ordering)
- `<Parallel maxConcurrency={n}>` — concurrent execution (multiple CUDA streams)
- `<Conditional test={(results) => ...}>` — data-dependent branching (no CUDA-graph equivalent — strictly more powerful)
- `<Map over={items} as="item">` — fan-out over a collection (batched dispatch)

### The two host configs

flowgraph ships two `HostConfig` implementations (`/workspace/@alkdev/flowgraph/src/host/`):

**`GraphologyHostConfig`** (`graphology.ts`) — renders the ujsx tree into a DAG, validates it against the operation graph (cycle detection via `hasCycle`, type-compatibility edges, topological sort). This is the *compile* step — like `cudagraph.capture()` building the graph from recorded ops, but declarative and validated before execution.

**`ReactiveHostConfig`** (`reactive.ts`) — renders the ujsx tree into a reactive execution structure where node statuses (`idle` → `waiting` → `ready` → `running` → `completed`/`failed`/`aborted`) are `@preact/signals-core` signals. `computePreconditions` checks all predecessors completed, `computeBlockedByFailure` propagates abort cascades, `registerStartEffect` reactively transitions `idle`→`ready` when preconditions are met (`/workspace/@alkdev/flowgraph/src/reactive/node-status.ts`). This is the *execute* step — like `cudagraph.launch()` but with dynamic status propagation.

Both run on the same ujsx reconciler + signals-core that POC 2 verified on QuickJS-NG.

### How this changes alknet-tensor

**The nn module hierarchy becomes flowgraph templates.** You don't port webgpu-torch's `nn_module.ts` `Module` class — you replace it with ujsx components:

```tsx
// Instead of webgpu-torch's imperative Module:
class ConvNet extends Module {
  constructor() {
    this.conv1 = Conv2d(1, 20, 5);
    this.conv2 = Conv2d(20, 20, 5);
  }
  forward(x) { return this.conv2(this.conv1(x).relu()).relu(); }
}

// alknet-tensor's declarative template:
const ConvNet = () => (
  <Sequential>
    <Operation name="tensor.conv2d" input={{ weight: w1, stride: 1 }} />
    <Operation name="tensor.relu" />
    <Operation name="tensor.conv2d" input={{ weight: w2, stride: 1 }} />
    <Operation name="tensor.relu" />
  </Sequential>
);
```

**The autograd graph *is* the ujsx tree.** Each `<Operation>` node knows its backward kernel (from the `OpSpec`'s `backward` expression). `backward()` walks the tree in reverse, dispatching backward kernels via the same flowgraph execution model. The `GradientContext` and `saveForBackward` bookkeeping from webgpu-torch's autograd (`src/autograd.ts`) becomes per-node state in the reactive host. The graph is declarative and inspectable before execution, not constructed as a side effect of running the forward pass — strictly cleaner than PyTorch's imperative autograd.

**Training loops are nested templates.** Composability is free because workflows are ujsx trees:

```tsx
const TrainingStep = ({ batch, labels }) => (
  <Sequential>
    <Operation name="model.forward" input={{ x: batch }} />
    <Operation name="loss.crossEntropy" input={{ predictions: "$.output", labels }} />
    <Operation name="model.backward" />  // walks the forward graph in reverse
    <Operation name="optim.step" input={{ params: "$.model.params", grads: "$.grads" }} />
  </Sequential>
);

const Epoch = ({ dataset }) => (
  <Map over={dataset} as="batch">
    <TrainingStep batch={batch.x} labels={batch.y} />
  </Map>
);
```

**CUDA-graphs-like capture and replay, but better:**

```
// PyTorch CUDA graph:
g = torch.cuda.CUDAGraph()
with torch.cuda.graph(g):
    out = model(input)
g.replay()  # re-run the captured graph

// alknet-tensor with flowgraph + ujsx:
const model = <Sequential>...</Sequential>;
// The ujsx tree IS the captured graph — declarative, not imperative capture.
// Replay = render(model) against the ReactiveHostConfig.
// The reconciler diffs the tree; only changed props re-dispatch.
// Conditional/Map allow dynamic structure that CUDA graphs can't express.
```

### Network-callable compute graphs

Since operations are `OperationSpec`s on the registry, a workflow template can mix local and remote ops:

```tsx
const Distributed = () => (
  <Parallel>
    <Operation name="tensor.matmul" input={{ a, b }} />       // local GPU
    <Operation name="remote.gpu1.matmul" input={{ a, b }} />   // peer GPU via irpc
    <Operation name="remote.gpu2.matmul" input={{ a, b }} />   // another peer
  </Parallel>
);
```

Same template, same execution model, different target. The `Parallel` host dispatches all three concurrently; the reactive status system tracks which completed; the results are collected. Distributed training is a workflow template, not a separate system.

### TSX authoring

flowgraph's components (`Operation`, `Sequential`, `Parallel`, `Conditional`, `Map`) are `UComponent` functions that return `{type, props, children}` — the exact ujsx element shape. Authoring in TSX is sugar for `h()` calls:

```tsx
<Sequential><Operation name="tensor.relu" /></Sequential>
// is sugar for:
h(Sequential, {}, h(Operation, { name: "tensor.relu" }))
```

The TSX→h transform is a build step (Rust crates: `swc_ecma_parser` / `oxc` can parse TSX and apply the standard JSX→h transform that ujsx's `jsx-runtime.ts` at `/workspace/@alkdev/ujsx/src/core/jsx-runtime.ts` is the target of). The runtime sees `UElement` trees either way; TSX is authoring ergonomics, not a runtime concern.

### Graph ops in Rust (petgraph), not JS (graphology)

flowgraph currently uses `graphology` + `graphology-dag` (~5400 lines of JS). The actual API surface flowgraph touches is small — ~15 distinct methods:

| graphology / graphology-dag API | petgraph equivalent |
|----------------------------------|---------------------|
| `new DirectedGraph()` | `DiGraph::new()` |
| `.addNode(id, attrs)` | `graph.add_node(attrs)` → returns `NodeIndex` |
| `.addEdgeWithKey(key, source, target, attrs)` | `graph.add_edge(source, target, attrs)` → returns `EdgeIndex` |
| `.dropEdge(source, target)` | `graph.remove_edge(edge_idx)` |
| `.hasNode(id)` | `graph.contains_node(idx)` |
| `.hasEdge(source, target)` / `.hasDirectedEdge(...)` | `graph.find_edge(n1, n2).is_some()` |
| `.nodes()` / `.edges()` | `graph.node_indices()` / `graph.edge_indices()` |
| `.order()` / `.size()` | `graph.node_count()` / `graph.edge_count()` |
| `.inDegree(id)` / `.outDegree(id)` | `graph.neighbors_directed(idx, Incoming/Outgoing).count()` |
| `.forEachNode(cb)` / `.forEachEdge(cb)` | `graph.node_indices().for_each(...)` |
| `hasCycle(graph)` | `petgraph::algo::is_cyclic_directed(graph)` |
| `topologicalSort(graph)` | `petgraph::algo::topological_sort(graph)` |
| `willCreateCycle(graph, source, target)` | add edge, check `is_cyclic_directed`, rollback — or check path exists from target to source |

Every graphology operation flowgraph uses maps to a one-line petgraph call. Porting the graph layer to Rust:

- Removes ~5400 lines of JS from the runtime (graphology + graphology-dag), shrinking the quickjs module load surface
- Makes graph operations native-speed (petgraph is already in the alknet dependency tree as a standard Rust crate)
- Enables graph validation to happen in Rust before the template is handed to the JS reactive host
- Keeps the ujsx tree authoring + reactive execution in JS (where the reconciler + signals-core handle the dynamic status propagation)

The `GraphologyHostConfig` becomes a Rust-backed host that builds a `petgraph::DiGraph` instead of a graphology `DirectedGraph`, exposing the graph to JS only for inspection (not manipulation). The `ReactiveHostConfig` stays in JS — it's signals and status propagation, which is what quickjs is good at.

### What this eliminates from the architecture

1. **`nn_module.ts` port** — replaced by flowgraph ujsx components. No `Module` base class, no `Parameter` wrapper, no `StateDict` serialization — those become flowgraph template inspection and registry queries.

2. **Imperative autograd recording** — replaced by declarative graph. The backward pass walks the ujsx tree, not a recorded tape. The graph is known before execution, not reconstructed after.

3. **graphology JS dependency** — replaced by petgraph in Rust. ~5400 lines of JS removed from the runtime.

4. **Custom graph validation** — flowgraph's `validateTemplate` already does cycle detection, type compatibility, topological ordering. This is graph validation that PyTorch and CUDA graphs don't have.

### What flowgraph *doesn't* provide (stays in alknet-compute)

- **The tensor ops themselves** — `tensor.matmul`, `tensor.conv2d`, `tensor.relu` etc. are still Rust-side wgpu compute kernels, exposed as `OperationSpec`s on the registry. flowgraph orchestrates them; it doesn't implement them.
- **Buffer management** — still Rust-owned `wgpu::Buffer` with `BufferId` handles in JS (the ~4-5 Rust ops from the architecture section above).
- **Shader codegen** — still `ShaderGenerator`/`WgslGenerator` (handlebars-rs) rendering `KernelSpec` → shader string. flowgraph is orthogonal to kernel compilation.
- **`gradcheck`** — finite-difference gradient verification, still a test harness operation.

---

## Updated Recommended Next POCs

In priority order:

1. **WGSL codegen probe** — write the `WgslGenerator` handlebars template (first `ShaderGenerator` impl) against `KernelSpec`, render all ~100 ops from `op_table.ts`, diff output against `getKernelShaderCode`'s output. If they match, the Rust codegen path is proven. Half-day exercise.

2. **`ExprCode` parser assessment** — read `src/expr.ts`, determine if the parser ports to Rust cleanly. If yes, stage 2 moves to Rust entirely. If no, stage 2 stays in JS and sends `KernelSpec` to Rust at init.

3. **End-to-end compute skeleton** — `alknet-compute` crate that depends on `alknet-runtime` (for the wgpu device + JS isolate + ops bridge) and `alknet-tensor` (for model loading), exposes `create_tensor` / `dispatch_kernel` / `read_tensor` to quickjs via the runtime's primitive compute dispatch, registers `tensor.matmul` as an `OperationSpec` on the runtime's registry, and runs a matmul via a flowgraph `<Sequential>` template. Proves the full stack (runtime + tensor + compute + flowgraph + ujsx) integrates. One day.

4. **`gradcheck` test harness** — implement finite-difference gradient verification as an operation, run against a subset of the op table with a flowgraph `<Sequential>` forward template and reverse-order backward template. Proves the autograd-via-flowgraph design. Half-day.

5. **petgraph host port** — port `GraphologyHostConfig` to a Rust-backed petgraph host, verify `validateTemplate` produces identical results against the existing test suite. Removes the graphology JS dependency. One day.

---

## References

- **alknet-runtime (substrate this builds on):** `docs/research/alknet-runtime/summary.md` — JS isolate, wgpu device, ops bridge, shared JS core, sandbox, primitive compute dispatch
- **alknet-tensor (format sibling):** `docs/research/alknet-tensor/metatensor-format.md` — pure-format binary tensor layout; `alknet-compute` registers the `load_model`/`stream_model` ops that bridge the format to wgpu buffers
- **Reference design (tensor):** `/workspace/webgpu-torch` — `src/op_spec.ts` (OpSpec schema), `src/op_table.ts` (452 lines, ~100 ops), `src/opgen.ts` (728 lines, op→kernel transform), `src/kernel.ts:299-375` (WGSL shader generation), `src/autograd.ts` (112 lines, gradient graph), `src/nn_module.ts` (467 lines, module hierarchy), `src/optim.ts` (204 lines, optimizers), `src/device_webgpu.ts` (GPU device + buffer pool with FinalizationRegistry)
- **Compute graph layer:** `/workspace/@alkdev/flowgraph` — `src/component/` (`Operation`, `Sequential`, `Parallel`, `Conditional`, `Map` — ujsx components that build the workflow template), `src/host/graphology.ts` (`GraphologyHostConfig` — renders template to DAG, validates), `src/host/reactive.ts` (`ReactiveHostConfig` — renders template to reactive execution structure), `src/reactive/node-status.ts` (`computePreconditions`, `computeBlockedByFailure`, `registerStartEffect` — signal-driven DAG execution), `src/graph/` (construction, validation, queries — graphology API surface to port to petgraph), `src/analysis/` (type-compat, ordering, workflow — graph validation)
- **Codegen infrastructure:** `/workspace/@alkimiadev/typebox-rs/src/codegen/` — `mod.rs` (`RustGenerator`, `TypeScriptGenerator`), `rust.rs` (handlebars → Rust structs), `typescript.rs` (handlebars → TS interfaces). The `ShaderGenerator` trait (with `WgslGenerator` as first impl) would be the third backend here.
- **wgpu shading-language support (multi-backend codegen):** https://docs.rs/wgpu/latest/wgpu/#shading-language-support — SPIR-V / GLSL / WGSL / naga-IR input languages; the `ShaderGenerator` trait is parameterized by these
- **Verified substrate (from alknet-desktop POCs):** `docs/research/alknet-desktop/poc-summary.md` — quickjs+wgpu+operations protocol all verified; llvmpipe software Vulkan confirmed as the headless backend
- **ujsx reconciler (verified on quickjs):** `/workspace/@alkdev/ujsx/src/host/{reconcile,config,fiber}.ts` — fiber-based reconciler with keyed child reconciliation, Value.Diff prop diffing, signal wiring
- **ujsx jsx-runtime (TSX→h target):** `/workspace/@alkdev/ujsx/src/core/jsx-runtime.ts` — the runtime that a TSX transform would emit calls to
- **typebox-rs (to be simplified with serde+jsonschema):** `/workspace/@alkimiadev/typebox-rs/` — `Cargo.toml` (handlebars v5, codegen feature), `src/schema.rs`, `src/builder.rs`
- **toolEnv (UDF sandbox precedent):** `/workspace/toolEnv/core/sandbox/` — `SandboxManager` with `allowFetch`/`allowFs` privilege flags, `@sebastianwessel/quickjs` WASM backend (alknet-compute uses native rquickjs via alknet-runtime instead)
- **Operations protocol (verified on quickjs):** `/workspace/@alkdev/operations/src/` — `registry.ts`, `call.ts`, `types.ts`, `validation.ts`, `response-envelope.ts`, `access.ts`
- **graphology API surface (to port to petgraph):** `~15 methods` used across `flowgraph/src/host/graphology.ts`, `flowgraph/src/graph/{construction,validation,queries}.ts`, `flowgraph/src/analysis/{type-compat,ordering,workflow}.ts` — all map 1:1 to `petgraph::DiGraph` + `petgraph::algo`
- **alknet ADRs (shared with alknet-desktop, via alknet-runtime):** `docs/architecture/decisions/` — ADR-005 (irpc), ADR-012 (stream model), ADR-013 (Rust canonical, alknet-call owns `OperationRegistry`), ADR-017 (call client + `from_call` adapter — the distributed-training mechanism)
- **wgpu clone (to be bumped to v29):** `/workspace/wgpu` (currently v24.0.5; compute API stable across versions, surface API changed around v25 but alknet-compute doesn't use surfaces — that's alknet-desktop's concern)