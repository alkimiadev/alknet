# alknet-tensor: Research Summary

**Status:** Early research — architecture direction established, no POCs yet. Derived from analyzing `webgpu-torch` as a reference design and the quickjs+wgpu verification from the alknet-desktop POCs.
**Date:** 2026-06-20
**Scope:** Captures the architectural direction for a Rust+wgpu tensor library with autograd, using QuickJS as a thin API/composition layer and WGSL compute shaders for execution. Documents what `webgpu-torch` established as a reference, how the architecture differs from a straight port, and what unknowns remain. Separate from `alknet-desktop` but shares the same verified substrate (quickjs + wgpu + the operations protocol).

---

## Executive Summary

`alknet-tensor` is a PyTorch-shaped tensor computation library built on Rust + wgpu, where the JS layer (QuickJS via rquickjs) is a thin API/composition surface and Rust owns memory, dispatch, and codegen. It is derived from the design of `webgpu-torch` (`/workspace/webgpu-torch`) — a pure-JS tensor + autograd library that runs entirely on the WebGPU compute pipeline — but is not a port of its code. webgpu-torch is the *reference design*; alknet-tensor is the *production architecture*.

The two completed alknet-desktop POCs (documented in `docs/research/alknet-desktop/poc-summary.md`) established the substrate this builds on:

1. **wgpu renders on llvmpipe (software Vulkan) with no physical GPU** — so tensor compute is testable on this OVH box right now, deployable to vast.ai GPU instances for production.
2. **QuickJS-NG runs the operations protocol (`@alkdev/operations` registry, call, envelopes, ACL, `buildCallHandler`)** — so every tensor op can be an `OperationSpec` on the registry, network-callable over irpc, same as any other operation.
3. **`typebox-rs` already has the handlebars codegen pattern** (`/workspace/@alkimiadev/typebox-rs/src/codegen/`) — `RustGenerator` and `TypeScriptGenerator` render typed schemas to target languages; a `WgslGenerator` is the same shape, rendering `KernelSpec` → WGSL shader strings.

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

The key architectural decision: **JS holds handles, Rust owns memory and dispatch.** This is the PyTorch model (Python holds handles, C++/CUDA owns memory) applied to QuickJS + wgpu.

### What lives in JS (QuickJS)

The thin API/composition layer. No tensor data, no GPU calls.

- **Tensor** = `{id: BufferId, shape: number[], dtype: string, requiresGrad: boolean, grad: Tensor | null}` — metadata only, the data is a Rust-owned `wgpu::Buffer`
- **Op table** — declarative `OpSpec` definitions (same schema as webgpu-torch's, possibly as TypeBox schemas for registry integration)
- **Autograd graph** — `GradientContext`, `AutoFunction`, backward bookkeeping. Pure metadata wiring.
- **nn module hierarchy** — `Module`, `Parameter`, `Sequential`, `Conv2d`, `Linear`, etc. Composition structure that builds the call graph.
- **Optimizer** — param groups, state, the `step()` loop. Calls Rust-side ops.
- **Custom kernel registration** — user writes WGSL string, calls `register_kernel(name, wgsl, input_specs, output_specs)`. Rust compiles and caches.
- **Operations registry integration** — each tensor op is an `OperationSpec` (verified on quickjs by POC 2). Built-in ops register at init; user ops register dynamically. All network-callable over irpc.

### What lives in Rust

Memory, dispatch, codegen. The execution layer.

- **Buffer manager** — `HashMap<BufferId, wgpu::Buffer>` with manual lifetime management. Replaces webgpu-torch's `FinalizationRegistry`-driven JS buffer pool with Rust-native resource management. No GC interaction, no weak refs, deterministic destruction.
- **Kernel compiler** — `wgpu::ShaderModule` creation from WGSL strings. Built-in kernels compiled at startup (or build time via handlebars codegen); custom kernels compiled on `register_kernel` call. Pipeline cache by shader hash.
- **Dispatch** — bind groups, compute pass encoding, `dispatchWorkgroups`, command submission. One Rust op per dispatch shape.
- **WGSL codegen** — `WgslGenerator` (handlebars-rs) renders `KernelSpec` → WGSL string. Same pattern as `typebox-rs`'s `RustGenerator` / `TypeScriptGenerator`. Build-time codegen for built-in ops; runtime compilation for custom kernels.
- **Readback** — `copyBufferToBuffer` to a mapped read buffer, return `ArrayBuffer` to JS. The only data-crossing op (explicit, like PyTorch's `.cpu()` / `.numpy()`).

### The Rust op surface

Minimal — ~4-5 high-level ops, not ~20 low-level GPU API calls:

| Op | Signature | Purpose |
|----|-----------|---------|
| `create_tensor` | `(data: ArrayBuffer, shape: number[], dtype: string) → BufferId` | Allocate a storage buffer, write initial data |
| `dispatch_kernel` | `(name: string, inputs: BufferId[], params: object, workgroup_count: [u, v, w]) → BufferId[]` | Look up compiled kernel, bind inputs, dispatch compute pass, return output buffer IDs |
| `register_kernel` | `(name: string, wgsl: string, input_specs: KernelInputSpec[], output_specs: KernelOutputSpec[]) → void` | Compile custom WGSL, cache by name |
| `read_tensor` | `(buffer_id: BufferId) → ArrayBuffer` | Copy buffer to mapped read buffer, return data to JS |
| `write_tensor` | `(buffer_id: BufferId, data: ArrayBuffer) → void` | Overwrite buffer contents from JS |

The data-crossing boundary is `read_tensor` / `write_tensor` only. A matmul on a 4096×4096 tensor is one `dispatch_kernel` call passing three `BufferId`s — the 64MB of floats never touch JS.

### The codegen pipeline

```
Build time:
  OpSpec[] (declarative, from op table)
    → opgen transform (opgen.ts logic, in Rust or JS)
    → KernelSpec[] (compute-pass descriptions)
    → WgslGenerator (handlebars-rs) renders each KernelSpec → WGSL string
    → wgpu pre-compiles each WGSL → ShaderModule (cached by name)

Runtime (built-in ops):
  JS calls dispatch_kernel("matmul", [a_id, b_id], params, count)
  → Rust looks up cached pipeline for "matmul"
  → binds buffers, dispatches, returns output BufferId

Runtime (custom kernels):
  JS calls register_kernel("my_op", wgsl_string, inputs, outputs)
  → Rust compiles WGSL via wgpu::ShaderModule
  → caches pipeline by name
  → subsequent dispatch_kernel("my_op", ...) uses the cached pipeline
```

The `WgslGenerator` is the natural third backend in `typebox-rs`'s codegen module:

```
typebox-rs/src/codegen/
├── mod.rs          — pub use RustGenerator, TypeScriptGenerator, WgslGenerator
├── rust.rs         — Schema → Rust structs (existing)
├── typescript.rs   — Schema → TS interfaces (existing)
└── wgsl.rs         — KernelSpec → WGSL shader (new)
```

The WGSL template encodes the scaffolding from webgpu-torch's `getKernelShaderCode` (`kernel.ts:299-375`): struct declarations, `@group(0) @binding(N)` declarations, `@compute @workgroup_size` header, conditional `@builtin` inclusion. One handlebars template with `{{#each inputs}}`, `{{#each outputs}}`, `{{#if uses_global_id}}` blocks.

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

The alknet-desktop research doc flagged "compositing 3D + 2D onto one surface" as an open unknown. Tensor compute sidesteps it entirely — compute pipelines have no surface, no swapchain, no present loop. The compositing complexity is a *render* problem; tensor ops are pure compute. This makes alknet-tensor structurally simpler than alknet-desktop despite being a "heavier" workload.

### 5. Cross-platform by construction, not configuration

wgpu's "one API, many backends" design means the same WGSL shaders and the same dispatch code run on Vulkan (Linux), Metal (macOS), DX12 (Windows), and llvmpipe (anywhere). No `#ifdef CUDA`, no "Linux is second-class", no platform-specific build matrix. The op table is WGSL strings; the execution is wgpu; the platform is whatever wgpu supports. Currently: everything.

---

## Relationship to alknet-desktop

alknet-tensor shares the verified substrate with alknet-desktop (quickjs + wgpu + the operations protocol) but is a separate concern:

| | alknet-desktop | alknet-tensor |
|---|---|---|
| **wgpu usage** | Render passes, surfaces, swapchain, compositing | Compute passes only — no surface, no swapchain |
| **GPU op surface** | ~25-40 ops (browser globals for three.js + surface management) | ~4-5 ops (create/dispatch/register/read/write) |
| **JS layer** | ujsx reconciler + HostConfig (3D + 2D UI composition) | Op table + autograd graph + nn module hierarchy |
| **Rust layer** | winit window + wgpu surface + three.js browser-env shims | wgpu buffer manager + kernel compiler + WGSL codegen |
| **Complexity driver** | The 3D+2D compositing and three.js shim surface | The autograd graph correctness and kernel codegen |
| **Network model** | Desktop worker dials head, renders UI | Tensor ops callable locally or over irpc; distributed training is ops on the registry |

They could share a crate (same quickjs runtime, same wgpu instance — a desktop app that also does tensor compute) or be separate crates (a pure compute server with no window). The operations registry is the shared seam — both register ops on the same protocol.

---

## Open Unknowns

### 1. Where does the op table live — Rust or JS?

If built-in ops are Rust-side (specs compiled at build time via handlebars `WgslGenerator`, kernels pre-registered), JS just calls `matmul(a, b)` and Rust looks up the compiled kernel. Fast, simple, fixed op surface.

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

## References

- **Reference design:** `/workspace/webgpu-torch` — `src/op_spec.ts` (OpSpec schema), `src/op_table.ts` (452 lines, ~100 ops), `src/opgen.ts` (728 lines, op→kernel transform), `src/kernel.ts:299-375` (WGSL shader generation), `src/autograd.ts` (112 lines, gradient graph), `src/nn_module.ts` (467 lines, module hierarchy), `src/optim.ts` (204 lines, optimizers), `src/device_webgpu.ts` (GPU device + buffer pool with FinalizationRegistry)
- **Codegen infrastructure:** `/workspace/@alkimiadev/typebox-rs/src/codegen/` — `mod.rs` (`RustGenerator`, `TypeScriptGenerator`), `rust.rs` (handlebars → Rust structs), `typescript.rs` (handlebars → TS interfaces). The `WgslGenerator` would be the third backend here.
- **Verified substrate (from alknet-desktop POCs):** `/workspace/@alkdev/alknet/docs/research/alknet-desktop/poc-summary.md` — quickjs+wgpu+operations protocol all verified; llvmpipe software Vulkan confirmed as the headless backend
- **typebox-rs (to be simplified with serde+jsonschema):** `/workspace/@alkimiadev/typebox-rs/` — `Cargo.toml` (handlebars v5, codegen feature), `src/schema.rs`, `src/builder.rs`
- **toolEnv (UDF sandbox precedent):** `/workspace/toolEnv/core/sandbox/` — `SandboxManager` with `allowFetch`/`allowFs` privilege flags, `@sebastianwessel/quickjs` WASM backend (alknet-tensor would use native rquickjs instead)
- **Operations protocol (verified on quickjs):** `/workspace/@alkdev/operations/src/` — `registry.ts`, `call.ts`, `types.ts`, `validation.ts`, `response-envelope.ts`, `access.ts`
- **alknet ADRs (shared with alknet-desktop):** `/workspace/@alkdev/alknet/docs/architecture/decisions/` — ADR-005 (irpc), ADR-012 (stream model), ADR-013 (Rust canonical), ADR-017 (call client contract)
- **wgpu clone (to be bumped to v29):** `/workspace/wgpu` (currently v24.0.5; compute API stable across versions, surface API changed around v25 but tensor compute doesn't use surfaces)