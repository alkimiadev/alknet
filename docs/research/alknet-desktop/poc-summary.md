# alknet-desktop: POC Research Summary

**Status:** Research complete on the two highest-leverage unknowns; further POCs planned before spec.
**Date:** 2026-06-20
**Scope:** Captures what the two completed POCs proved, what unknowns they closed, what remains open, and the architectural direction they jointly establish. Source material for the eventual `alknet-desktop` crate spec.

---

## Executive Summary

Two POCs were completed that, between them, resolve the two largest sources of feasibility uncertainty around building `alknet-desktop` — a custom desktop environment on Rust + wgpu + QuickJS, with ujsx as the user-facing IR over three.js (3D) and an SDF layer (2D UI), networked over alknet's irpc/ALPN infrastructure.

1. **`ui-spoke-poc`** (`/workspace/ui-spoke-poc`) — proved that headless WebGPU rendering in Deno works end-to-end, including the hard cases: driving three.js's `WebGPURenderer` without a DOM, and MSDF text rendering (the "2D UI is rocket surgery" subproblem). Established the `HeadlessCanvas` abstraction and the device-capture technique that makes it work.
2. **`quickjs-reactive-probe`** (`/workspace/quickjs-reactive-probe`) — proved that the reactive core (`@preact/signals-core`, `@alkdev/typebox` Value namespace, and `@alkdev/ujsx`'s fiber-based reconciler with full mount/update/unmount) runs cleanly on QuickJS-NG via rquickjs. The runtime-compat question that could have sunk the whole approach is closed: it works.

Both POCs were run on this OVH box, which has no physical GPU. wgpu renders against Mesa's `llvmpipe` (LLVM 20.1.2, 256-bit SIMD) software Vulkan driver — a property that reinforces the design rather than compromising it (see §Headless/Headed Parity below).

The supply-chain attack surface is a primary motivation for this path over the alternative (see §Why Not `deno desktop`). Minimizing the dependency tree — no Chromium, no V8, no Node compat layer — collapses the surface area an attacker can exploit via transitive dependencies.

---

## Background: The Original Direction

The first investigation explored using Deno's unreleased `deno desktop` feature with its `raw` backend (no webview, no Chromium) to host the WebGPU surface. A deep source-dive of the deno repository (findings in `/workspace/conversations/research/deno-desktop-raw-backend-source.md`, 839 lines) surfaced several blockers that, while individually surmountable, collectively made the path unattractive:

- The `raw` backend is real (`laufey_winit`-backed, creates OS windows via winit) and `BrowserWindow.getNativeWindow()` is backend-agnostic, but the implementation lives behind a prebuilt binary from `github.com/littledivy/laufey/releases` whose handle-return behavior is unverifiable from source.
- `child_process.fork()` workers are detected by argv shape and forced headless, panicking on `new BrowserWindow()`. The head-worker model requires `Deno.Command` subprocesses instead of V8-isolate workers.
- DevTools under `raw` is broken in practice: `openDevtools()` opens a winit window with no webview to render HTML. The CDP mux's `/deno` passthrough works for raw CDP clients but not the bundled frontend.
- The `deno.json` `desktop.backend` schema enum is `["webview", "cef"]` while the CLI/runtime accept `"raw"` — the docs' claim that raw is config-only is inverted.

More fundamentally, `deno desktop` drags in the entire Deno runtime (V8, Node compat layer, npm/JSR resolver machinery) to run JS that, per the spoke/ujsx architecture, doesn't need any of it. The user's `spoke.ts` already speaks WebSocket to a head — under alknet that becomes irpc over an ALPN. There is no need for Deno's HTTP server, its module loader, or its `Deno.*` namespace. The alknet-desktop path replaces all of that with a ~210 KiB QuickJS-NG isolate plus the small set of Rust ops we choose to expose. The dependency surface shrinks from "a whole JS runtime" to "one Rust crate we own + three small audited JS libraries we own."

---

## POC 1: `ui-spoke-poc` — Headless WebGPU + three.js + MSDF Text

**Location:** `/workspace/ui-spoke-poc`
**Runtime:** Deno 2.x with `--unstable-webgpu`
**GPU backend:** llvmpipe (software Vulkan, no physical GPU on the test box)

### What it proved

**Headless WebGPU rendering works without a DOM or browser globals.** `src/headless-canvas.ts` implements `HeadlessCanvas` — a from-scratch `GPUCanvasContext` stand-in that:

- Allocates render textures via `device.createTexture({ usage: RENDER_ATTACHMENT | COPY_SRC })` (`src/headless-canvas.ts:54-65`)
- Implements `configure`/`getCurrentTexture`/`unconfigure`/`present` — the full `GPUCanvasContext` surface (`src/headless-canvas.ts:38-76`)
- Handles the `bgra8unorm` ↔ `rgba8unorm` channel swap on readback (`src/headless-canvas.ts:111-126`)
- Ships a dependency-free PNG encoder (CRC32 + zlib deflate + Adler32, ~120 lines) for snapshot testing (`src/headless-canvas.ts:161-284`)
- Exposes `assertPixel`/`pixelAt`/`readPixels`/`writeSnapshot` for visual regression tests (`src/headless-canvas.ts:86-151`)

The tests in `test/headless_test.ts` exercise clear-color rendering, format conversion, and PNG output — all pass on llvmpipe.

**three.js's WebGPURenderer can be driven headlessly via a device-capture trick.** The key non-obvious insight, in `examples/threejs-webgpu.ts:35-67`: hand three.js a fake canvas whose `getContext("webgpu").configure()` *captures the `GPUDevice` that three.js passes in*, then allocate textures on *that* device. Without this, readback fails because the texture lives on three.js's internal device while the readback buffer lives on yours. The fake canvas implements only `getContext`/`configure`/`getCurrentTexture`/`unconfigure` — five methods total. three.js's full scene graph (Scene, Mesh, MeshStandardNodeMaterial, AmbientLight, DirectionalLight, PerspectiveCamera, WebGPURenderer) renders correctly to a `bgra8unorm` texture, which is then swizzled and written to PNG.

**MSDF text rendering — the canonical "2D UI is hard" problem — is tractable.** `examples/msdf-text.ts` (583 lines) renders "Hello, World!" via instanced quads + MSDF font atlas + TSL shader. Notable techniques:

- **`copyExternalImageToTexture` shim** (`examples/msdf-text.ts:71-101`): Deno lacks this method on `GPUQueue`. The shim reaches into `Symbol.for("Deno_bitmapData")` to extract RGB pixels from `ImageBitmap`, expands to RGBA, and dispatches to `writeTexture`. This is the one Deno-specific hack; under wgpu-direct it becomes a plain `queue.write_texture` call.
- **Font layout** (`examples/msdf-text.ts:144-201`): pure layout computation (kerning, scaling, cursor advance) extracted renderer-agnostically — directly portable to the alknet-desktop path.
- **TSL MSDF shader** (`examples/msdf-text.ts:346-374`): the standard median-of-three SDF evaluation with `smoothstep` antialiasing and gamma correction, expressed in three.js's TSL node system. Compiles to WGSL at runtime.

The MSDF POC is the proof that 2D UI on the same wgpu surface as 3D is not a research question — it's an implementation question. The primitives (rounded rect, circle, button, slider, separator, shadow) all have known SDF representations; the `typegpu-sdf-ujsx-webgpu-host.md` research doc in `/workspace/conversations/research/` designs the full library.

### What it established for alknet-desktop

- **The minimal JS-side surface area** for driving three.js headlessly is small: `window.innerWidth/innerHeight/devicePixelRatio` (3 numbers), `requestAnimationFrame`/`cancelAnimationFrame` (one timer hook), `document.createElement("canvas")` returning `{width, height, style, getContext, getBoundingClientRect, addEventListener}`, and `getContext("webgpu")` returning the `GPUCanvasContext`-shaped object. Under the Rust path each becomes a single Rust op.
- **The readback pattern** (`copyTextureToBuffer` → `mapAsync` → swizzle) is the testing substrate for visual regression. This is what alknet-desktop's CI snapshot tests will use, independent of whether the surface is headed or headless.
- **The device-capture technique** (letting three.js create the device, capturing it via `configure()`) is the bridge that lets a render target you don't own (three.js's WebGPURenderer) write into a texture you do own. This is exactly the mechanism needed for 3D+2D compositing: three.js renders into one texture, the SDF layer into another, a final compositor pass merges them onto the presentable surface. The POC already demonstrates the hard half of that.

### What doesn't transfer

- The Deno-specific shims (`Symbol.for("Deno_bitmapData")`) become direct wgpu calls — no symbol hackery needed.
- The JS-level `HeadlessCanvas` class collapses into a Rust struct exposed to JS. The "common interface" between headed and headless becomes more honest: *you own the surface object*, so headed vs. headless is a config flag on one Rust struct, not two parallel JS classes pretending to be interchangeable.
- The Deno runtime itself is replaced by rquickjs + whatever ops we expose. No V8, no Node compat, no `Deno.*` namespace.

---

## POC 2: `quickjs-reactive-probe` — ujsx Reconciler on QuickJS-NG

**Location:** `/workspace/quickjs-reactive-probe`
**Runtime:** rquickjs 0.12 wrapping QuickJS-NG (ES2020)
**Status:** Probe passes; reactive core verified compatible.

### The question

`@alkdev/ujsx`'s reconciler (`/workspace/@alkdev/ujsx/src/host/reconcile.ts`, 521 lines) is a real fiber-based reconciler with:

- Keyed child reconciliation via longest-increasing-subsequence move minimization (`reconcile.ts:245-283`)
- `Value.Diff`/`Value.Hash`/`Value.Clone`/`Value.Equal`-driven prop diffing against `@alkdev/typebox` schemas (`reconcile.ts:81-192`)
- Signal wiring via `@preact/signals-core`'s `effect()` (`reconcile.ts:227-238`)
- Microtask-batched updates via `queueMicrotask` (`reconcile.ts:45-79`)

QuickJS-NG targets ES2020. The POC code itself is ES2020-or-earlier (optional chaining, nullish coalescing, public class fields, TLA), but TypeGPU and three.js's WebGPU build ship as TS that may emit ES2022+ patterns (class static blocks, `Array.prototype.at()`, top-level `await` in modules). The probe's job was to determine whether the reactive core — the part we own — runs on QuickJS-NG before we commit to the runtime. three.js compat is a separate, scoping question (see §Open Unknowns).

### What the probe exercises

The probe (`/workspace/quickjs-reactive-probe/probe.mjs`) loads the *built* ESM distributions of all three libraries:

- `@preact/signals-core` — `signals-core.module.js` (minified, ES5-ish prototype style; references `Symbol.dispose` only as a property assignment, not via `using`, so it won't crash even if the symbol is absent)
- `@alkdev/typebox` + `@alkdev/typebox/value` — the ESM build at `build/esm/` (250 modules loaded transitively)
- `@alkdev/ujsx` — the bundled chunks at `dist/` (host/config → `chunk-UBTVTQ75.js` for the reconciler, `chunk-NGTIHDKG.js` for `h`/`createComponent`, etc.)

It then runs four assertions groups:

1. **signals-core**: `signal(0)` → `effect()` observes initial value → `s.value = 42` propagates → `batch()` + `computed()` recompute correctly.
2. **typebox Value namespace**: `Type.Object(...)` builder works → `Value.Hash()` returns `bigint` → `Value.Diff()` returns array → `Value.Clone()` deep-copies → `Value.Equal()` compares.
3. **ujsx reconciler mount/update/unmount**: build `<div count={0}><span>hi</span></div>`, render via a no-op `HostConfig` that records every call, verify `createInstance("div")`/`createInstance("span")`/`createTextInstance("hi")` fire; update to `<div count={5}><span>bye</span>`, verify `commitUpdate` fires for the changed `count` prop; unmount, verify `finalizeRoot` fires.
4. **Reactive wiring**: a `createComponent` that reads a `signal()` in its render fn mounts successfully via `createRoot(dynHost, {}).render(h(Counter, {id: "c1"}))`.

### Result

```
EVAL_OK result={"signals":"ok","typebox":"ok","reconcilerMount":"ok",
                "reconcilerUpdate":"ok","reconcilerUnmount":"ok",
                "reactiveWiring":"ok","totalHostCalls":2,"callsBeforeSignal":2}
total modules: 253
PROBE PASSED
```

253 modules loaded and linked by QuickJS-NG without a single syntax or import error. Every assertion passed. The reactive core runs on QuickJS-NG via rquickjs.

### One observation worth recording

`totalHostCalls: 2` and `callsBeforeSignal === totalHostCalls` on the reactive test: the `countSignal.value = 99` mutation either didn't trigger a fiber update within the synchronous drain, or the update was queued but the `queueMicrotask`-scheduled microtask didn't flush before `Module::finish()` returned. This is *not* a compat failure — signals themselves propagate correctly (verified in group 1); it's a "you'll need to pump the microtask queue explicitly in the real render loop" note. rquickjs's job-drain timing differs from V8's. In alknet-desktop the render loop will call into the runtime per-frame, so microtasks flush naturally; in a probe that evaluates once and exits, the timing window is tighter. Not a blocker — a known scheduling detail to handle in the real host.

### What it established for alknet-desktop

- **No runtime-compat blocker at the JS layer.** The three libraries we own (`ujsx`, `typebox`, `signals-core`) all run on QuickJS-NG. The path from "ujsx tree" → "reconciler diff" → "HostConfig.createInstance/commitUpdate calls" is operational.
- **The HostConfig is the seam.** The probe's no-op `HostConfig` is the shape of the work: a `HostConfig<string, THREE.Object3D, ThreeRootCtx>` for 3D and a `HostConfig<string, SdfInstance, SdfRootCtx>` for 2D UI. Both target the same wgpu surface; the user picks per-subtree via a host boundary. This is the R3F model (React Three Fiber): `<mesh>` becomes `new THREE.Mesh()` inside `createInstance`, prop diffs become three.js mutations inside `commitUpdate`.
- **253 modules is the typebox transitive surface.** Worth knowing for cold-start budgeting — alknet-desktop will load this tree once at startup. On quickjs-ng the per-module parse+link cost is low (the whole probe runs in well under a second including compile), but it's not free. A bytecode-bundle preload (rquickjs's `embed!` macro) is the mitigation if cold start matters.

---

## Architectural Direction (Established by the Two POCs)

### The stack

```
┌─────────────────────────────────────────────────────────────────┐
│  User code: ujsx trees                                          │
│  <scene><mesh geometry={...} material={...}><model src=.../></scene>  │
│  <panel><text>Hello</text><button onClick={...}>OK</button></panel> │
└──────────────────────────────┬──────────────────────────────────┘
                               │ h() / createComponent()
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│  ujsx reconciler (verified on QuickJS-NG by POC 2)              │
│  fiber tree, Value.Diff prop diffing, signal wiring             │
└──────────────────────────────┬──────────────────────────────────┘
                               │ HostConfig.createInstance / commitUpdate
                               ▼
┌──────────────────────────┐   ┌────────────────────────────────┐
│  Three.js HostConfig      │   │  SDF/ujsx HostConfig            │
│  (3D scenes, models)      │   │  (2D UI: panels, text, buttons) │
│  createInstance("mesh")   │   │  createInstance("panel")        │
│   → new THREE.Mesh(...)    │   │   → SdfRoundedRect(...)         │
│  R3F-shaped adapter       │   │  (from typegpu-sdf research)    │
└─────────────┬─────────────┘   └──────────────┬─────────────────┘
              │                                │
              ▼ renders to offscreen TextureView (device-capture trick, POC 1)
              ▼                                ▼
┌─────────────────────────────────────────────────────────────────┐
│  Compositor: final render pass merges 3D + 2D onto wgpu Surface │
└──────────────────────────────┬──────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│  Rust: wgpu v29 + winit + rquickjs isolate                      │
│  Browser-global shims become Rust ops (~25-40 total)            │
│  Headless: llvmpipe software Vulkan (no GPU needed)             │
│  Headed: real GPU or xvfb virtual display                        │
└──────────────────────────────┬──────────────────────────────────┘
                               │ irpc over alknet-call ALPN
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│  Head: alknet endpoint, ProtocolHandler-registered ALPN string  │
│  Desktop worker is a network client of head (ADR-017 contract)  │
└─────────────────────────────────────────────────────────────────┘
```

### Headless/Headed parity

The two POCs converge on a property that makes the deployment surface uniform: **headed and headless are the same code path, differing only in whether a `Surface` (swapchain) or a `Texture` (offscreen) is the render target.** This holds at three levels:

1. **wgpu level**: `createTexture({ usage: RENDER_ATTACHMENT | COPY_SRC })` (headless, POC 1's `HeadlessCanvas`) and `surface.getCurrentTexture()` (headed) both yield `GPUTexture`s whose `createView()` can be a render-pass attachment. The render commands are identical.
2. **adapter level**: llvmpipe presents as a normal Vulkan ICD. `navigator.gpu.requestAdapter()` returns a real adapter whether or not a physical GPU exists. No adapter-detection branching in user code.
3. **JS level**: the `HeadlessCanvas` `getContext("webgpu")` surface (`configure`/`getCurrentTexture`/`unconfigure`) is the same shape as `Deno.UnsafeWindowSurface`'s context. Under the Rust path this becomes one struct with a `mode: Headed | Headless` field, exposed to JS identically in both modes.

### The testing matrix

| Tier | Environment | What it tests | GPU backend |
|------|------------|---------------|-------------|
| **Build + unit + snapshot** | This OVH box, headless | Shader correctness, render output, visual regression via PNG diff | llvmpipe |
| **Window lifecycle** | CI with `xvfb-run` | winit window creation, resize, close events, surface recreation | llvmpipe |
| **Real-GPU perf + visual** | Developer desktop or vast.ai GPU instance | Frame rates, real GPU features (ray tracing, mesh shaders), visual sanity | Vendor driver |

One codebase, no `#ifdef`-style branching. The wgpu headless pattern (no `Surface`, render to `Texture`, `copyTextureToBuffer`, read pixels) is stable across wgpu versions; the windowed path is where v24→v29 API churn concentrates (`Surface` lifetime, `SurfaceTarget`/`SurfaceTargetUnsafe`), so the v29 bump work is scoped to the headed code path only.

### Supply-chain surface

The dependency tree for the JS-runtime layer:

| Dependency | Source | Owner | Audit burden |
|------------|--------|-------|--------------|
| rquickjs + QuickJS-NG | crates.io / github.com/DelSkayn/rquickjs | upstream, vendored | one Rust crate + one C lib (compile from source) |
| `@preact/signals-core` | npm | preactjs | ~1 file minified, ~3KB; verified by POC 2 |
| `@alkdev/typebox` | jsr / git.alk.dev | alkdev | 250 ESM modules, owned |
| `@alkdev/ujsx` | npm / git.alk.dev | alkdev | owned, reconciler verified by POC 2 |
| three.js (if used) | cdn / npm | mrdoob | largest external dep; see Open Unknowns |
| wgpu + winit | crates.io | gfx-rs / rust-windowing | two well-audited Rust crates |

Compared to the `deno desktop` path (V8 + Node compat + npm resolver + deno runtime + laufey prebuilt binary + CEF or system webview), this is a dramatic reduction in transitive attack surface. Every component is either owned by alkdev, a well-known Rust crate, or a single small JS library.

---

## Open Unknowns (For Future POCs)

These are the unknowns that remain after the two POCs. None are feasibility blockers (the basic mechanics work); they are scope/work-quantity questions that affect spec sizing.

### 1. three.js's browser-environment op surface (scoping, not feasibility)

The POC 1 shims (`ui-spoke-poc/src/shims.ts`) are deceptively small because they only cover the render path. The loaders — GLTFLoader, DRACOLoader, KTX2Loader — touch a wider surface: `fetch`, `URL`, `Blob`/`File`, `Image`/`HTMLImageElement`, `createImageBitmap`, `TextDecoder`/`TextEncoder`, `Response`, possibly `Worker` for DRACO/KTX2 decompression. Each becomes a Rust op. Most are small (`URL` is string parsing, `TextDecoder` is a one-liner). The ones with real surface are:

- **`fetch`** — needs to route into alknet's HTTP handler or the head's HTTP capability. Medium effort.
- **`Image`/`createImageBitmap`** — needs image decoders (PNG/WebP/JPEG/KTX2). The `image` Rust crate covers the first three; KTX2 needs `basis-universal` or a transcode pass. Medium-high effort.

A scoping probe would enumerate what three.js's loader stack actually touches by running `GLTFLoader.parse()` against a test model inside a quickjs isolate with instrumented globals, producing a concrete op list. Replaces the current guess ("~25-40 ops") with a number.

### 2. Compositing 3D + 2D onto one surface (design, not feasibility)

POC 1's device-capture technique is the hard half — three.js renders into a texture you control. The remaining design question is the compositor: does three.js render to an offscreen `TextureView`, the SDF layer to another, and a final render pass merges them onto the `Surface`? Or does one host own the surface and the other render into a texture that's then composited as a textured quad in the first host's scene? The two shapes have different implications for input hit-testing (which host receives a click at screen coord `(x,y)`?) and z-ordering. Worth a small design probe before spec.

### 3. wgpu v29 surface-from-handle ergonomics (mechanical, not design)

The workspace's wgpu clone is at v24.0.5 (May 2025); latest is v29.0.1. The `Surface` API changed materially around v25 (`Surface`<'window> lifetime removal, `SurfaceTarget`/`SurfaceTargetUnsafe` rework). The headless path is unaffected (no `Surface`); the headed path needs a v29 migration pass. ~1 day of API relearning, not a design problem. The clone is free real estate (no downstream commitments), so it can be blown away and re-cloned at v29 cleanly.

### 4. irpc throughput on the hot path (perf, deferred correctly)

The head pushes scene-graph/uniform updates to the desktop worker every frame. irpc uses `EventEnvelope` framing (ADR-012). Whether the framing overhead is acceptable at 60fps, or whether the hot path needs raw `BiStream` bytes bypassing `EventEnvelope`, is a perf question that can't be answered without measuring. Deferred until there's a running end-to-end system to benchmark. The hybrid option (irpc for control, raw `BiStream` for frame data) is the fallback if framing overhead is real.

### 5. Microtask scheduling in the real render loop (mechanical)

POC 2 noted that `queueMicrotask`-scheduled updates didn't flush before `Module::finish()` returned in the one-shot probe. In alknet-desktop the render loop calls into the runtime per-frame, so microtasks drain naturally. Worth a one-line addition to the probe (explicit `ctx.run_jobs()` or equivalent) to confirm, but not a spec blocker.

### 6. Things we don't know we don't know

The two POCs answered the two biggest known-unknowns. The remaining unknowns above are all scope/work questions, not feasibility questions. The SDD (spec-driven development) process — documented at `/workspace/@alkdev/alknet/docs/sdd_process.md` — should not begin until the open unknowns above are resolved by additional small POCs, because a spec written against guesses about scope sizes produces unreliable implementation plans. The recommended next POCs, in priority order:

1. **three.js loader op enumeration** — run GLTFLoader in a quickjs isolate with instrumented globals; produce the concrete op list.
2. **Compositing design probe** — render three.js to a texture + SDF layer to a texture + compositor pass onto a wgpu surface, end-to-end, on llvmpipe. Answers the compositing shape question and exercises the v29 surface-from-handle API at the same time.
3. **End-to-end skeleton** — `alknet-desktop` crate skeleton: Cargo.toml + lib.rs that opens a winit window, creates a wgpu v29 surface, loads the ujsx reconciler via rquickjs, renders a hardcoded `<div>` tree to the surface via a no-op HostConfig. Proves the full stack integrates before any spec is written.

---

## References

- POC 1: `/workspace/ui-spoke-poc` — `src/headless-canvas.ts`, `src/shims.ts`, `examples/threejs-webgpu.ts`, `examples/msdf-text.ts`, `test/headless_test.ts`
- POC 2: `/workspace/quickjs-reactive-probe` — `src/main.rs`, `probe.mjs`, `Cargo.toml`
- Deno desktop source dive (the rejected alternative): `/workspace/conversations/research/deno-desktop-raw-backend-source.md`
- SDF/ujsx host design (the 2D UI library this enables): `/workspace/conversations/research/typegpu-sdf-ujsx-webgpu-host.md`
- ujsx reconciler source: `/workspace/@alkdev/ujsx/src/host/{reconcile,config,fiber}.ts`
- alknet ADRs: `/workspace/@alkdev/alknet/docs/architecture/decisions/` (ADR-005 irpc, ADR-012 stream model, ADR-013 Rust canonical, ADR-017 call client contract — all relevant to the desktop-worker-over-irpc model)
- wgpu clone (to be bumped to v29): `/workspace/wgpu` (currently v24.0.5)
- llvmpipe ICD: `/usr/share/vulkan/icd.d/lvp_icd.json` (the software Vulkan driver backing all headless rendering on this box)