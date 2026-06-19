# Project state — @gdforj/restate-sdk-unison

> This file is maintained by Claude. Update it whenever goals shift, decisions are made, work is completed, or the roadmap changes.

## Goal

Build a Restate SDK for the Unison programming language, published as `@gdforj/restate-sdk-unison` on Unison Share. This enables Unison programs to participate in durable distributed systems powered by the Restate runtime.

## Status

**Phase: MVP complete** — All five layers typechecked and pushed to the UCM codebase.

Completed:
- ✅ Rust FFI wrapper (`crates/restate-sdk-unison-native`) — full C ABI over `CoreVM`
- ✅ Core types (`scratch/01_types.u`) — `Serde`, `Input`, `Target`, `Future`, `AwaitResult`, `NotifValue`, etc.
- ✅ Native FFI bindings (`scratch/02_native.u`) — 47 definitions wrapping all C functions
- ✅ Ability layer (`scratch/03_ability.u`) — `Restate.Ctx` + `Restate.State` abilities, `Restate.Vm.runHandler`
- ✅ HTTP endpoint (`scratch/04_endpoint.u`) — `Restate.Endpoint.serve`, discovery + invocation routing
- ✅ Greeter example (`scratch/05_example.u`) — stateful virtual object with counter

## Architecture decisions

### 1. Protocol binding: native FFI to `sdk-shared-core`

Unison calls the `CoreVM` Rust state machine via Unison's native C FFI (`FFI.DLL.openByName` / `FFI.DLL.getSymbol`). A thin Rust wrapper crate (`crates/restate-sdk-unison-native`) wraps `restate-sdk-shared-core` with `#[no_mangle] extern "C"` functions and compiles as `crate-type = ["cdylib"]`.

**Integer handles**: The FFI uses `u64` integer handles instead of `*mut c_void` raw pointers, because Unison has no `Ptr.isNull` for null checking. `0` signals allocation failure.

**JSON boundary**: Complex structs (response head, awakeable handle, etc.) are JSON-encoded on the Rust side and decoded by Unison's JSON decoder, avoiding struct layout issues across the FFI boundary.

### 2. Handler API: two abilities

```
ability Restate.Ctx where     -- ctx.input, ctx.sleep, ctx.call, ctx.send, ctx.run, ctx.await, ctx.awakeable, ctx.completeAwakeable
ability Restate.State where   -- state.get, state.set, state.clear, state.clearAll, state.keys
```

A single `Restate.Vm.runHandler` interpreter handles both with one `handle expr with go` block where `go : Request {Restate.Ctx, Restate.State} Bytes ->{IO, Exception} Bytes`. The key was adding an explicit `Request` type annotation on `go` — without it, Unison infers the handler as pure and rejects IO calls inside the arms.

### 3. Serialization: explicit `Serde` records

```
type Serde a = { encode : a -> Bytes, decode : Bytes -> Either Text a }
```

No forced JSON dependency. Handlers operate on raw `Bytes`; the user chooses the serialization format.

### 4. HTTP endpoint

Uses `unison_http_16_1_0` server library:
- `GET /discover` → JSON discovery response (service names, handler names, types)
- `POST /:service/:handler` → invocation: create VM, feed body, run handler, collect output, return response

Path parsing: `match HttpRequest.uri req with URI _ _ path _ _ -> Path.segments path` gives `[svcName, hdlName]`.

## Project structure

```
restatedev-sdk-unison/
├── CLAUDE.md                       # How to work in this repo (conventions, MCP, git)
├── PROJECT.md                      # Project state (this file)
├── flake.nix                       # Dev shell: unison-ucm + Rust toolchain
├── .mcp.json                       # Unison MCP server
├── .claude/settings.json           # enableAllProjectMcpServers: true
├── crates/
│   └── restate-sdk-unison-native/  # Rust cdylib: C ABI over CoreVM
│       ├── Cargo.toml
│       └── src/lib.rs
└── scratch/                        # Unison scratch files (all typechecked & pushed)
    ├── 01_types.u                  # Core types: Serde, Input, Target, Future, etc.
    ├── 02_native.u                 # FFI bindings to the native library
    ├── 03_ability.u                # Restate.Ctx + Restate.State abilities + runHandler
    ├── 04_endpoint.u               # HTTP server: discovery + invocation dispatch
    └── 05_example.u                # Greeter virtual object example
```

UCM codebase at `~/.config/unisonlanguage/` (project: `scratch`, branch: `main`).

## Reference repos (read-only, locally cloned)

- `/home/gdforj/public/restatedev-sdk-shared-core` — `CoreVM` Rust crate
- `/home/gdforj/public/restatedev-sdk-typescript` — TypeScript SDK reference

## Next steps

### Integration testing
- Run `nix develop` then `ucm run Restate.Example.main` to start the server
- Register the endpoint with a local Restate runtime and send test invocations
- Verify durable execution replays correctly

### Polish before publishing
- Add `Restate.Serde.json` convenience (JSON encode/decode using `unison_json_1_4_2`)
- Add `ctx.sleep` duration helpers
- Add handler type annotations to distinguish `SERVICE` vs `VIRTUAL_OBJECT` handlers
- Add `Workflow` handler support (promise get/complete)
- Error response encoding (proper content-type + body for terminal failures)
- Publish to `@gdforj/restate-sdk-unison` on Unison Share

## Key Unison syntax discoveries

- **Ability handlers**: `handle expr with f` where `f : Request {A, B} r ->{IO, Exception} r` — the handler is a named `cases` function, NOT inline cases. The `{ op args -> k }` brace syntax is used in arms; `handle k result with f` resumes the continuation.
- **Handler effects**: Without an explicit `Request` type annotation on `go`, Unison infers it as pure and rejects IO in arm bodies.
- **Reserved keyword**: `handle` cannot be a record field name — use `rawHandle`.
- **Lambda bindings**: `=` bindings inside lambda arguments cause parse errors; inline the expression.
- **Record constructors**: `type Foo.Bar = { x : Int }` creates constructor `Foo.Bar.Bar`.
