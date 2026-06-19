# Project state — @gdforj/restate-sdk-unison

> This file is maintained by Claude. Update it whenever goals shift, decisions are made, work is completed, or the roadmap changes.

## Goal

Build a Restate SDK for the Unison programming language, published as `@gdforj/restate-sdk-unison` on Unison Share. This enables Unison programs to participate in durable distributed systems powered by the Restate runtime.

## Status

**Phase: Project setup** — CLAUDE.md and PROJECT.md written, dev environment configured (flake.nix, Unison MCP server). No code written yet.

## Architecture decisions

### 1. Protocol binding: native FFI to `sdk-shared-core`

Unison calls the `CoreVM` Rust state machine via Unison's native C FFI (`FFI.openByName` / `FFI.getSymbol`). A thin Rust wrapper crate (`crates/restate-sdk-unison-native`) wraps `restate-sdk-shared-core` with `#[no_mangle] extern "C"` functions and compiles as `crate-type = ["cdylib"]`. The wrapper crate is our addition — `sdk-shared-core` has no C ABI today.

C ABI surface (planned): VM lifecycle (`vm_create`, `vm_destroy`), input (`vm_notify_input`, `vm_notify_input_closed`), output (`vm_take_output`), syscalls (`vm_sys_run`, `vm_sys_call`, `vm_sys_sleep`, `vm_sys_state_get`, etc.), `vm_do_await`.

### 2. Handler API: Unison abilities

Four abilities compose at the handler type signature, giving compile-time enforcement of what each handler type can do:

```
ability Restate where          -- all handlers: call, send, run, sleep, awakeable, rand
ability RestateState where     -- object/workflow exclusive: get, set, clear
ability RestateStateRead where -- object/workflow shared: get, stateKeys (read-only)
ability RestateWorkflow where  -- workflow: getPromise, completePromise
```

Handler ability sets:
- Service: `'{Restate, IO}`
- Object exclusive: `'{Restate, RestateState, IO}`
- Object shared: `'{Restate, RestateStateRead, IO}`
- Workflow run: `'{Restate, RestateState, RestateWorkflow, IO}`
- Workflow shared: `'{Restate, RestateStateRead, RestateWorkflow, IO}`

### 3. Serialization: explicit `Serde` records

```
type Serde a = { encode : a -> Bytes, decode : Bytes -> Either Text a }
```

No forced JSON dependency. SDK ships `Restate.Json.serde` using `@unison/json` as a convenience. Handlers are sealed at registration by pairing with `Serde` values, producing a uniform `Bytes ->{IO} Bytes`.

## Project structure

```
restatedev-sdk-unison/
├── CLAUDE.md                     # How to work in this repo (conventions, MCP, git)
├── PROJECT.md                    # Project state (this file)
├── flake.nix                     # Dev shell: unison-ucm + Rust toolchain (TODO: add Rust)
├── .mcp.json                     # Unison MCP server (nix run nixpkgs#unison-ucm -- mcp)
├── .claude/settings.json         # enableAllProjectMcpServers: true
├── crates/
│   └── restate-sdk-unison-native/  # Rust cdylib: C ABI over CoreVM (TODO: create)
│       ├── Cargo.toml
│       └── src/lib.rs
├── scratch/                      # Unison scratch files (.u) for iterating (TODO: create)
└── docs/
    └── unison/
        └── ffi.md                # Unison FFI reference
```

Unison library code lives in the UCM codebase (managed by UCM). Scratch files are the iteration surface.

## Reference repos (read-only, locally cloned)

- `/home/gdforj/public/restatedev-sdk-shared-core` — `CoreVM` Rust crate; see `src/lib.rs` for VM trait and `docs/sdk-integration.md` for integration guide
- `/home/gdforj/public/restatedev-sdk-typescript` — most complete SDK reference; see `packages/libs/restate-sdk/src/context.ts` for API and `context_impl.ts` for VM loop

## Roadmap

### Step 1 — Rust FFI wrapper (next)
- Create `crates/restate-sdk-unison-native/` with `Cargo.toml` and `src/lib.rs`
- Expose C ABI over `CoreVM` (lifecycle, input/output, syscalls, `do_await`)
- Wire into `flake.nix` so the `.so` is available in the dev shell

### Step 2 — Unison ability definitions
- Define `Restate`, `RestateState`, `RestateStateRead`, `RestateWorkflow` abilities
- Define `Serde a` type
- Define the ability handler that drives the `CoreVM` loop via FFI

### Step 3 — Service/Object/Workflow abstractions
- Define `ServiceDef`, `ObjectDef`, `WorkflowDef` types
- `handler`, `exclusive`, `shared`, `run` registration functions that seal `Serde` pairs

### Step 4 — HTTP endpoint
- Implement the HTTP server that handles Restate's discovery and invocation protocol
- Service discovery endpoint (`GET /` returning service manifest)
- Handler dispatch

### Step 5 — Examples and testing
- Greeter service example
- Counter virtual object example
- Integration test against a local Restate runtime

## Open questions

- Which HTTP library on Unison Share to use for the endpoint server?
- How to handle the Restate discovery protocol (service manifest format)?
