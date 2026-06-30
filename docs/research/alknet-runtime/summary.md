# alknet-runtime: JS + wgpu Substrate (Research Summary)

**Status:** Concept design derived from the alknet-desktop and alknet-tensor POCs/research. No POCs yet for the extracted substrate itself; the substrate's components are individually verified by the two existing POCs.
**Date:** 2026-06-30
**Scope:** The generalized QuickJS-NG + wgpu runtime that serves as the shared substrate for `alknet-desktop` (render) and `alknet-compute` (tensor compute). Owns the JS isolate, the wgpu device, the operations-protocol bridge, and the shared JS core bundle. Consumers layer their HostConfigs and op surfaces on top. This doc captures the boundary, what moves out of the consumer crates into the runtime, the layering against alknet-call, and the open unknowns for the extracted substrate specifically.

---

## Executive Summary

Two POCs (`alknet-desktop` and the `alknet-tensor` architecture) independently arrived at the same substrate: rquickjs wrapping QuickJS-NG, wgpu for device access, and the `@alkdev/operations` protocol as the JS↔Rust bridge. The desktop POC verified the reactive core + operations protocol on QuickJS-NG (271 modules load and link cleanly); the tensor architecture verified that wgpu compute on llvmpipe (software Vulkan, no physical GPU) is genuinely useful compute — WGSL compiled to optimized SIMD beats JS for any non-trivial workload, and the same WGSL runs at full GPU speed when a GPU is present.

Rather than have both consumer crates re-implement rquickjs setup, the ops bridge, the shared JS core bundle, sandbox/privilege flags, and wgpu device acquisition, this substrate is extracted as `alknet-runtime`. Consumers keep what's genuinely theirs: the render surface (desktop), the buffer manager + kernel codegen + autograd (compute), the binary format (tensor). The runtime owns the *always-present* layer that every consumer needs.

The key property that makes wgpu unconditional rather than a feature flag: **wgpu on llvmpipe is a better-than-JS compute layer on every box, with no GPU required.** A UDF host with no render needs still benefits from wgpu — compute-bound UDFs (string processing, hashing, image transforms, signal processing, ML ops) can register WGSL kernels and dispatch them on llvmpipe, getting SIMD-speed compute on a CPU-only box and full GPU speed when one is present. There is no deployment scenario where you'd want to *not* have wgpu available. The "UDF-only host" case isn't a no-wgpu case — it's a compute host that happens to not render.

---

## The Boundary

### What alknet-runtime owns

- **rquickjs isolate lifecycle.** Creation, module resolver, `embed!` bytecode preload of the shared JS core bundle, microtask/job pumping (the POC-2 scheduling note — `queueMicrotask`-scheduled updates need explicit `ctx.run_jobs()` in one-shot probes but flush naturally in the per-frame render loop — is handled here once, not per-consumer).
- **wgpu device + adapter acquisition.** Unconditional — every consumer gets a device. Adapter request is parameterized by intent (`RenderIntent::None` for compute-only, `RenderIntent::Surface` for desktop) so the runtime requests the right adapter features without consumers owning acquisition logic. llvmpipe presents as a normal Vulkan ICD; no adapter-detection branching.
- **The operations-protocol bridge.** The Rust↔JS call surface: Rust ops exposed to JS as `envProxy`, JS UDFs registered and callable from Rust, `ResponseEnvelope` plumbing, ACL enforcement hook, the bidirectional `buildCallHandler` counterpart on the Rust side. This is the seam that alknet-call's `CallClient`/`OperationRegistry` speak to.
- **The shared JS core bundle.** The exact 271-module set verified by POC-2: `@preact/signals-core`, `@alkdev/typebox`, `@alkdev/ujsx` reconciler, `@alkdev/operations`, `@alkdev/pubsub`, `@logtape/logtape`. Shipped as one embedded `embed!` bundle. Consumers add their own modules on top (flowgraph, three.js, custom UDF code).
- **Sandbox / privilege model.** The `allowFetch` / `allowFs` / `envProxy` shape from `toolEnv`, generalized. A UDF host with `allowFetch: false` / `allowFs: false` and only registered operations exposed is a real sandbox — native rquickjs (OS-level isolation via process boundaries) instead of the WASM-QuickJS path `toolEnv` v1 used.
- **Primitive wgpu compute dispatch.** Compile a shader module, create buffer, dispatch compute pass, readback. These primitives are general — any UDF can use them, not just ML. This is the "wgpu compute as a better-than-JS layer everywhere" capability, exposed at the runtime layer so every consumer gets it.
- **The cold-start budget.** 271 modules is the baseline. The runtime owns the `embed!` bundle and the load-once-at-startup cost. Consumers adding modules (flowgraph, three.js) layer on top and own their own cold-start contributions.

### What moves out of the consumer crates into the runtime

| Concern | Was in (consumer) | Now in (runtime) |
|---|---|---|
| rquickjs isolate setup + `embed!` | desktop POC-2, tensor architecture | runtime |
| wgpu device acquisition | both | runtime |
| Operations protocol Rust bridge | both | runtime |
| Shared JS core bundle loading | both | runtime |
| Sandbox / privilege flags | toolEnv (WASM path) | runtime (native path) |
| Microtask/job pumping | desktop POC-2 noted it | runtime (one place) |
| Primitive compute dispatch (shader/buffer/dispatch) | tensor architecture's ~5 ops | runtime exposes primitives; compute layers the tensor-shaped ops on top |

### What stays in the consumer crates

- **`alknet-desktop`** — winit, `Surface`/swapchain (the wgpu v29 surface-API migration is entirely here, not in runtime), three.js browser-global shims (~25-40 ops), Three `HostConfig` + SDF `HostConfig`, the 3D+2D compositor, the irpc-to-head client (ADR-017 contract). Uses runtime's device; owns the render surface. Optionally depends on `alknet-compute` when an app does in-process ML on the same device.
- **`alknet-compute`** — `BufferId`-handle buffer manager, the `OpSpec`/`KernelSpec` op table, `ShaderGenerator` (handlebars codegen, WGSL first / SPIR-V / GLSL / naga-IR later), the tensor-shaped high-level ops (`create_tensor`/`dispatch_kernel`/`register_kernel`/`read_tensor`/`write_tensor`), autograd-via-flowgraph, `gradcheck`, distributed training over irpc. Uses runtime's primitive compute dispatch; owns the tensor abstractions and codegen.
- **`alknet-tensor`** (the metatensor format) — pure-format, no runtime dep. Schema-driven binary layout, `compute_offsets`, mmap via `memmap2`, QUIC/BiStream per-tensor mapping, ujsx `<Struct>/<Field>/<Tensor>` authoring → TypeBox schema + OffsetMap. Can be used by a pure-Rust model server with no JS runtime at all; runtime integration (a `load_model` op) is registered by `alknet-compute`, not by the format crate.

---

## Layering Against alknet-call

Per ADR-013, alknet-call owns the canonical `OperationSpec`, `OperationRegistry`, `CallClient`, and the adapter contract. Per ADR-017, `CallClient` opens connections, shares the dispatch loop, and the connection is symmetric after establishment — both sides can call each other. The `from_call` adapter imports remote operations into a local registry.

The layering:

```
alknet-call          (canonical op types, CallClient, adapter contract — no JS, no wgpu)
   ▲
   │ depends on
   │
alknet-runtime       (JS isolate + wgpu device + ops bridge into alknet-call registry)
   ▲
   │ depends on
   │
alknet-compute       (tensor ops + codegen + autograd, registers on runtime's registry)
alknet-desktop       (render surface + three.js shims, registers on runtime's registry)

alknet-tensor        (pure format, no runtime dep — sibling, used by compute)
```

- **alknet-runtime depends on alknet-call.** The runtime's ops bridge is the Rust-side counterpart to `@alkdev/operations`'s `buildCallHandler`: it produces `HandlerRegistration` bundles that register into an `OperationRegistry` (the same registry type alknet-call owns). UDFs authored in JS and registered via `envProxy` become `OperationSpec`s on the registry, network-callable via `CallClient`. This is the "QuickJS UDF host convergence" the desktop POC flagged — made concrete by depending on alknet-call, not reimplementing the protocol.
- **alknet-compute and alknet-desktop depend on alknet-runtime.** They get the JS isolate, the wgpu device, and the ops bridge for free. They register their op surfaces (tensor ops, render ops) on the runtime's registry, which is the same registry alknet-call dispatches through.
- **alknet-tensor is a sibling, not a child.** The format has no JS or wgpu dependency — it's pure Rust (`memmap2`, `jsonschema`, `serde`). A model server using only the format doesn't pull the runtime. `alknet-compute` depends on both `alknet-runtime` and `alknet-tensor` and registers the `load_model`/`stream_model` ops that bridge them.

The consequence: a node running `alknet-compute` as a `CallClient`-connected worker exposes its tensor ops as `External` operations on the registry, discovered via `services/list` + `services/schema` by any peer. The peer's `from_call` adapter imports them. Distributed training is a `flowgraph` template mixing local and `from_call`-imported remote ops — same template, same execution model, different target. The runtime is what makes a worker "an operation host that happens to do ML" rather than a special ML-specific endpoint.

---

## The wgpu Substrate: Why It's Unconditional

The observation that drives wgpu being a hard dependency rather than a feature flag:

- **llvmpipe compiles WGSL to optimized SIMD.** On a box with no physical GPU (this OVH server, CI runners, edge devices), wgpu's llvmpipe backend translates WGSL shaders to LLVM IR, which LLVM optimizes to native SIMD. The result is compute that beats hand-written JS for any non-trivial workload — verified empirically by writing a SHA-256 shader in WGSL and measuring it against the JS implementation in Deno's WebGPU (which itself uses wgpu under the hood). The GPU-less case is not a "wgpu doesn't work" case; it's a "wgpu gives you SIMD compute for free" case.
- **The same WGSL runs on a real GPU at full speed.** A kernel written and tested on llvmpipe deploys unchanged to a vast.ai GPU instance. No `#ifdef CUDA`, no platform-specific build matrix. wgpu's "one API, many backends" (Vulkan / Metal / DX12 / llvmpipe) means the deployment target is a runtime concern, not a code concern.
- **This makes WGSL kernels a first-class UDF optimization layer.** Any UDF that's compute-bound — not just ML ops — can register a WGSL kernel via the runtime's primitive compute dispatch and get SIMD-speed compute on a CPU-only box, full GPU speed when present. The toolEnv WASM-QuickJS baseline (sandbox with no GPU access) is strictly weaker: native rquickjs + wgpu gives you the same sandbox boundary *plus* a portable compute accelerator.

The Rust-SIMD-would-be-faster caveat is real but mostly orthogonal to the design: hand-tuned native Rust SIMD beats llvmpipe, but (a) llvmpipe-on-any-box beats JS-on-any-box, (b) the same WGSL is full-GPU-speed on a real device, and (c) the cases where you'd reach for native Rust SIMD are the cases you control at build time — which is exactly what `alknet-compute`'s build-time codegen pipeline produces (precompiled kernels for the built-in op table). You get both layers: hand-tuned Rust where it matters, portable WGSL for everything else, and UDF-authored WGSL for the long tail of compute-bound ops that aren't worth a Rust crate.

### Shading-language support (multi-backend codegen)

wgpu accepts SPIR-V, GLSL, WGSL (default), and naga-IR as shader input languages (`wgpu` crate features). The codegen pipeline in `alknet-compute` (`ShaderGenerator`) should be parameterized by target language, not hardcoded to WGSL:

- **WGSL first** — existing reference material (`webgpu-torch`'s op table), the author's familiarity, and the default wgpu feature make it the right starting point.
- **SPIR-V / GLSL / naga-IR later** — the handlebars-rs codegen templates can be retargeted; `KernelSpec` is language-agnostic data, only the final template render is language-specific. Keeping `ShaderGenerator` as a trait with `WgslGenerator` as the first impl preserves the option without committing to it now.

The `ShaderGenerator` trait lives in `alknet-compute` (it's a tensor-compute concern: rendering `KernelSpec` to a shader string), not in `alknet-runtime` (runtime only compiles whatever shader string the consumer hands it via the primitive dispatch surface).

---

## The Shared JS Core Bundle

The 271 modules POC-2 verified are the runtime's baseline:

| Module | Purpose | Owner | Audit burden |
|---|---|---|---|
| `@preact/signals-core` | Reactive primitives | preactjs | ~1 file, ~3KB |
| `@alkdev/typebox` + `/value` | Schema system (250 modules transitively) | alkdev | owned |
| `@alkdev/ujsx` | Reconciler (fiber tree, Value.Diff prop diffing, signal wiring) | alkdev | owned |
| `@alkdev/operations` | Operations protocol (registry, call, envelopes, ACL) | alkdev | owned |
| `@alkdev/pubsub` | `Repeater` async iterators | alkdev | owned |
| `@logtape/logtape` | Logging (with neutral `#util` subpath) | logtape | small |

This is the **runtime's** bundle — the layer every consumer needs. Consumer-specific bundles layer on top:

- **`alknet-compute`** adds `@alkdev/flowgraph` (the `<Operation>`/`<Sequential>`/`<Parallel>`/`<Conditional>`/`<Map>` components + `GraphologyHostConfig` + `ReactiveHostConfig`). The graphology JS dependency is the candidate for the petgraph port (a separate `alknet-compute` concern, not a runtime concern).
- **`alknet-desktop`** adds three.js + the user-facing ujsx HostConfigs (Three for 3D, SDF for 2D UI). three.js is the largest external dep and the one with the open scoping unknown (loader op enumeration — see `alknet-desktop/poc-summary.md` §Open Unknowns).

The runtime owns the cold-start budget for the core bundle. Consumers own their own cold-start additions. The POC-2 note about `embed!` bytecode preload as the cold-start mitigation applies to the runtime's bundle; consumers can use the same technique for their additions.

---

## The Sandbox / Privilege Model

`toolEnv` (`/workspace/toolEnv/core/sandbox/`) proved the concept with WASM-QuickJS (`@sebastianwessel/quickjs` + `@jitl/quickjs-ng-wasmfile-release-sync`): `SandboxManager.executeScript(code, env, consoleHandler, timeout)` with `allowFetch` / `allowFs` privilege flags and an `envProxy` exposing only registered operations. The runtime generalizes this to native rquickjs:

- **Same privilege shape.** `allowFetch: bool`, `allowFs: bool`, `envProxy: { [opName]: fn }`. A UDF host with `allowFetch: false` / `allowFs: false` and only registered operations exposed is a real sandbox — the UDF cannot reach the network or filesystem except through the ops the host chose to expose.
- **Different isolation boundary.** WASM-QuickJS gives browser-compatible maximal isolation at slower speed; native rquickjs gives OS-level isolation (process boundary) at native speed. The operations protocol is the same in both — UDFs authored for the WASM path run unchanged on the native path. The choice is a deployment-time decision, not a design-time one.
- **The runtime owns the privilege enforcement point.** `allowFetch`/`allowFs` become gates on whether the runtime exposes `fetch`/`fs` Rust ops to the JS isolate. The `envProxy` is the only way the UDF reaches the outside world when both are false.

This is directly relevant to both consumers:
- **`alknet-desktop`** is a UDF host whose operations happen to include "render this ujsx tree" and "handle this input event" — the desktop isn't special, it's an operation host with a GPU.
- **`alknet-compute`** is a UDF host whose operations happen to include "dispatch this kernel" and "read this buffer" — same model, different op surface.

An LLM-authored operation (model architecture, tool composition, custom kernel) runs in the same sandbox with the same privilege model regardless of which consumer hosts it.

---

## Open Unknowns (Substrate-Specific)

These are the unknowns introduced by *extracting* the substrate. The consumer-specific unknowns (compositing, three.js loader op surface, autograd correctness, etc.) stay in their respective docs.

### 1. Does alknet-runtime own the `OperationRegistry`, or share one from alknet-call?

alknet-call owns the canonical `OperationRegistry` type (ADR-013/017). The runtime's ops bridge registers into *an* `OperationRegistry` — but is it a fresh one the runtime owns, the consumer's global one, or a peer-scoped overlay (ADR-028/029)? ADR-017 §1 flagged this as a two-way door with a security dimension: sharing the global registry exposes local capabilities to a remote peer; a peer-scoped subset must filter by capability remote-safety, not just operation name. **Recommendation:** the runtime takes an `Arc<OperationRegistry>` (or a reference to one) at construction — the consumer (or alknet-call's `CallClient`) owns the registry; the runtime bridges into it. Keeps the registry authority in alknet-call where ADR-013 put it.

### 2. Device intent and the render/compute split

The runtime acquires the wgpu device unconditionally, but the *adapter features* differ: compute-only needs no surface support, desktop needs `RenderIntent::Surface` (swapchain, present). The split is a config flag on one acquisition path, not two paths. The open question is the exact `DeviceRequest` shape — does it take an enum (`DeviceIntent::ComputeOnly | DeviceIntent::RenderSurface { window_handle }`) or a builder (`DeviceRequest::new().with_render_surface(window)`)?

**Recommendation:** enum first; builder if it grows. The v29 surface-API migration scope lives entirely in the `RenderSurface` arm, which is only constructed by `alknet-desktop`. Compute-only consumers never touch it.

### 3. Microtask scheduling as a runtime API

POC-2 noted that `queueMicrotask`-scheduled updates didn't flush before `Module::finish()` returned in the one-shot probe. In a per-frame render loop (desktop) or per-dispatch compute loop (compute), microtasks drain naturally because the runtime is re-entered. The runtime should expose an explicit `pump_jobs()` / `drain_microtasks()` API so one-shot hosts (UDF execution that evaluates and exits) and tests can flush deterministically. The open question is whether this is a public API or an internal detail the consumer's event loop calls implicitly.

**Recommendation:** public API. Consumers with their own event loop (desktop's raf loop, compute's dispatch loop) call it implicitly; one-shot hosts and tests call it explicitly. Same mechanism, two call sites.

### 4. JS bundle composition

The runtime embeds the 271-module core. Consumers add their own bundles (flowgraph, three.js). The open question is the composition mechanism: does the runtime expose a `load_bundle(bytes)` API that consumers call at init, or are consumer bundles separate `embed!` invocations in the consumer crate that share the same isolate?

**Recommendation:** the runtime owns the isolate and the core bundle; consumers register additional module-resolver entries that point at their own embedded or file-loaded bundles. The module resolver is the composition point. A consumer that wants to ship flowgraph bundles its own `embed!` and registers the modules with the runtime's resolver.

### 5. Schema module placement

The runtime needs the jsonschema + `TypeDef:*` custom keywords for the ops bridge (operation specs are JSON Schemas). `alknet-tensor` (the format) also needs jsonschema + offset computation, with no runtime dep. The open question: does the schema module (jsonschema wrapper + custom keywords) live in the runtime, in a tiny separate `alknet-schema` crate, or duplicated?

**Recommendation:** tiny separate `alknet-schema` crate (jsonschema + custom keyword impls + offset computation), depended on by both `alknet-runtime` and `alknet-tensor`. Avoids the runtime pulling in tensor's offset logic, and avoids tensor pulling in the runtime's JS machinery. The split cost is one small crate; the win is the format crate stays pure-Rust.

### 6. WASM-QuickJS vs native-rquickjs as a runtime backend

`toolEnv` v1 used WASM-QuickJS (browser-compatible, maximal isolation, slower). The native rquickjs path is the default for alknet-runtime (OS-level isolation via process boundaries, native speed). The open question is whether the runtime should support *both* backends behind a trait, so the same UDF code runs in a browser sandbox (WASM) or a native sandbox (rquickjs) depending on deployment.

**Recommendation:** defer. The operations protocol is the same in both; the privilege model is the same. The runtime targets native rquickjs first; a WASM backend is a later addition if a browser-hosted UDF use case materializes. Don't generalize before there's a second consumer.

---

## Recommended Next POCs (Substrate-Specific)

In priority order, these are POCs for the *extracted runtime* — distinct from the consumer POCs (compositing design, three.js loader enumeration, WGSL codegen, autograd-via-flowgraph) which stay in their respective docs.

1. **Runtime skeleton** — `alknet-runtime` crate: `Cargo.toml` + `lib.rs` that constructs an rquickjs isolate, embeds the 271-module core bundle via `embed!`, acquires a wgpu device on llvmpipe with `DeviceIntent::ComputeOnly`, exposes the primitive compute dispatch ops (`compile_shader`/`create_buffer`/`dispatch`/`readback`) to JS, and runs a hardcoded WGSL matmul from JS via `envProxy`. Proves the substrate integrates end-to-end. One day.

2. **Ops bridge probe** — extend the runtime skeleton to register a JS UDF (`test.echo`) on the `OperationRegistry`, call it from Rust via `env.invoke()`, and call a Rust op from JS via `envProxy`. Verifies the bidirectional bridge against alknet-call's registry type. Half-day on top of the skeleton.

3. **Cold-start measurement** — instrument the runtime skeleton's startup: module load + link time, `embed!` bytecode preload vs source-parse, wgpu device acquisition time on llvmpipe. Produces the budget numbers the consumer crates need for their cold-start accounting. Half-day.

4. **`pump_jobs()` API verification** — the POC-2 microtask note, made concrete: add `pump_jobs()` to the runtime, verify a one-shot UDF execution flushes microtasks deterministically with an explicit call, verify a per-frame loop flushes implicitly. Confirms the scheduling model. Half-day.

5. **Sandbox privilege enforcement** — construct the runtime with `allowFetch: false` / `allowFs: false`, verify a UDF that attempts `fetch()` or `fs.readFile()` fails cleanly (op not exposed), verify a UDF that calls a registered op via `envProxy` succeeds. Confirms the privilege gate. Half-day on top of the ops bridge probe.

---

## Relationship to the Consumer Crates

| | alknet-runtime | alknet-desktop | alknet-compute | alknet-tensor |
|---|---|---|---|---|
| **Owns** | JS isolate, wgpu device, ops bridge, shared JS core, sandbox, primitive compute dispatch | winit, surface/swapchain, three.js shims, Three/SDF HostConfigs, compositor, irpc-to-head client | Buffer manager, op table, `ShaderGenerator`, tensor ops, autograd-via-flowgraph, `gradcheck`, distributed training | Binary format, offset computation, mmap, QUIC stream mapping, ujsx layout authoring |
| **Depends on** | alknet-call, alknet-schema (if extracted) | alknet-runtime (+ alknet-compute if in-process ML) | alknet-runtime, alknet-tensor, alknet-schema | alknet-schema (pure-format path) |
| **wgpu usage** | Device acquisition + primitive compute dispatch | Render passes, surfaces, swapchain, compositing | Compute passes only — no surface | None (pure format) |
| **JS layer** | Shared core bundle (271 modules) | + three.js + Three/SDF HostConfigs | + flowgraph + reactive execution host | None (pure format) |
| **Complexity driver** | The extraction boundary itself (what's truly shared vs accidentally coupled) | 3D+2D compositing, three.js shim surface | Autograd correctness, kernel codegen, distributed training | Offset computation correctness, mmap safety, blob indirection |
| **Network model** | Ops bridge into alknet-call registry (UDFs become network-callable) | Desktop worker dials head, renders UI (ADR-017 client contract) | Tensor ops on registry, distributed via `from_call` (ADR-017) | QUIC per-tensor streams (format property, not runtime) |

---

## What This Eliminates

1. **Duplicate rquickjs setup.** Both consumer crates would have re-implemented isolate creation, module resolver, `embed!` bundling, microtask pumping. One implementation in the runtime.

2. **Duplicate ops-protocol bridge.** Both consumer crates would have re-implemented the Rust↔JS call surface and `ResponseEnvelope` plumbing. One implementation in the runtime, bridging into alknet-call's registry.

3. **Duplicate shared JS core loading.** Both consumer crates would have embedded the same 271-module bundle. One bundle in the runtime; consumers layer on top.

4. **Duplicate sandbox/privilege enforcement.** `toolEnv`'s privilege model, generalized to native rquickjs once. Both consumers inherit it; neither re-implements the gate.

5. **Duplicate wgpu device acquisition.** Both consumer crates would have acquired the device (with different intent flags). One acquisition path in the runtime, parameterized by intent.

6. **The "is wgpu a feature flag" question, resolved.** wgpu is unconditional. Every consumer gets device access and primitive compute dispatch. The "UDF-only host with no GPU" case is reframed as "a compute host that happens to not render" — it still benefits from wgpu on llvmpipe.

---

## References

- **alknet-desktop POCs (substrate verification):** `docs/research/alknet-desktop/poc-summary.md` — quickjs-reactive-probe (271 modules verified on QuickJS-NG), ui-spoke-poc (headless WebGPU + three.js + MSDF text on llvmpipe)
- **alknet-compute architecture (consumer):** `docs/research/alknet-compute/architecture-summary.md` — tensor compute engine building on this runtime + alknet-tensor
- **alknet-tensor format (sibling):** `docs/research/alknet-tensor/metatensor-format.md` — pure-format binary tensor layout, no runtime dep
- **alknet-call (lower layer):** `docs/architecture/decisions/013-rust-canonical-implementation.md` (Rust canonical, adapter traits in alknet-call), `docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md` (`CallClient`, `from_call`, registry overlay model)
- **toolEnv (sandbox precedent):** `/workspace/toolEnv/core/sandbox/` — `SandboxManager`, `SandboxEnv`, `SandboxOptions` with `allowFetch`/`allowFs` privilege flags
- **quickjs-reactive-probe (the probe itself):** `/workspace/quickjs-reactive-probe` — `src/main.rs`, `probe.mjs`, `Cargo.toml`
- **ui-spoke-poc (the headless WebGPU probe):** `/workspace/ui-spoke-poc` — `src/headless-canvas.ts`, `src/shims.ts`, `examples/threejs-webgpu.ts`, `examples/msdf-text.ts`
- **wgpu shading-language support (multi-backend codegen):** https://docs.rs/wgpu/latest/wgpu/#shading-language-support — SPIR-V / GLSL / WGSL / naga-IR input languages
- **webgpu-torch (reference for compute consumer):** `/workspace/webgpu-torch` — `src/op_spec.ts`, `src/op_table.ts`, `src/opgen.ts`, `src/kernel.ts`, `src/autograd.ts`
- **flowgraph (compute graph layer, used by alknet-compute):** `/workspace/@alkdev/flowgraph` — `src/component/`, `src/host/{graphology,reactive}.ts`
- **Operations protocol (verified on quickjs):** `/workspace/@alkdev/operations/src/` — `registry.ts`, `call.ts`, `types.ts`, `validation.ts`, `response-envelope.ts`, `access.ts`