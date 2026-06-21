# Project state — @gdforj/restate-sdk-unison

> This file is maintained by Claude. Update it whenever goals shift, decisions are made, work is completed, or the roadmap changes.

## Goal

Build a Restate SDK for the Unison programming language, published as `@guillaumedesforges/restate-sdk-unison` on Unison Share. This enables Unison programs to participate in durable distributed systems powered by the Restate runtime.

## Status

**Phase: Stage 5 complete — published to Unison Share.**

Published at: https://share.unison-lang.org/@guillaumedesforges/restate-sdk-unison/code/main

Package `@gdforj/restate-sdk-unison` (UCM codebase, branch `main`):
- ✅ Core types + abilities — in package, Stage 1 tests green
- ✅ `encodeDiscovery` — tested; bugs fixed: protocol versions 5–7, `protocolMode: "REQUEST_RESPONSE"`, EXCLUSIVE handler type for virtual objects
- ✅ `flatHeaders` — tested (red→green)
- ✅ `pathSegments` — tested (red→green)
- ✅ `Serde` round-trip — tested
- ✅ FFI smoke tests (`Restate.Vm.tests.stage2.*`) — `new`, `free`, `notifyInput`, `notifyInputClosed`, `getResponseHead` all pass
- ✅ VM protocol round-trip (`Restate.Vm.tests.stage3.*`) — synthetic frames, echo handler, output extraction
- ✅ HTTP endpoint (`Restate.Endpoint.serve`) — serves discovery + handles invocations
- ✅ Greeter example — full round-trip against Restate 1.6.2; state persists across calls
- ✅ Integration tests (`scripts/test-integration.sh`) — all 6 tests pass (discovery, direct echo, Restate registration, Greeter count=1, count=2, admin API confirm)

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
├── README.md                       # Project README (installation, quick example, concepts)
├── CLAUDE.md                       # How to work in this repo (conventions, MCP, git)
├── PROJECT.md                      # Project state (this file)
├── Cargo.toml                      # Workspace root (members: crates/restate-sdk-unison-native)
├── Cargo.lock
├── flake.nix                       # Dev shell: unison-ucm + Rust toolchain
├── .mcp.json                       # Unison MCP server
├── .claude/settings.json           # enableAllProjectMcpServers: true
├── .github/
│   ├── workflows/ci.yml            # Build + symbol-check on push/PR (ubuntu-latest)
│   └── workflows/release.yml       # Build + GitHub Release on v* tags (Linux + macOS ARM)
├── scripts/
│   └── test-integration.sh         # 6-test integration harness (requires nix develop .#integration)
└── crates/
    └── restate-sdk-unison-native/  # Rust cdylib: C ABI over CoreVM
        ├── Cargo.toml
        └── src/lib.rs
```

UCM codebase at `~/.config/unisonlanguage/` (project: `@gdforj/restate-sdk-unison`, branch: `main`).

## Reference repos (read-only, locally cloned)

- `/home/gdforj/public/restatedev-sdk-shared-core` — `CoreVM` Rust crate
- `/home/gdforj/public/restatedev-sdk-typescript` — TypeScript SDK reference

## Next steps

### Phase 2 — TDD rebuild as a proper Unison package

Methodology: red-green-refactor throughout. No code is considered done without a passing test. See CLAUDE.md for the full testing methodology.

**Stage 1 — Unit tests for pure functions ✅**
- `encodeDiscovery` output matches expected string
- Path parsing extracts `["greeter", "greet"]` from a synthetic `HttpRequest`
- `flatHeaders` round-trips a known header map
- `Serde` encode/decode round-trips

**Stage 2 — FFI smoke tests ✅** (run with `nix develop .#default`)
- `Restate.Vm.new [("content-type", "application/vnd.restate.invocation.v5")]` returns a valid handle
- `notifyInput` + `notifyInputClosed` do not error
- `getResponseHead` returns status 200
- `free` does not crash

**Stage 3 — VM protocol tests (synthetic journal bytes) ✅**
- `Restate.Vm.testInvocationBytes` (Unison) + `restate_test_invocation_bytes` (Rust): encode StartMessage + InputCommandMessage frames from raw bytes
- `Restate.Vm.testExtractOutputValue` (Unison) + `restate_test_extract_output_value` (Rust): parse OutputCommandMessage value.content from collected output
- Echo handler driven via `runHandler`; output matches input — full VM protocol round-trip verified

**Stage 4 — Integration test against a local Restate runtime ✅**

Script: `scripts/test-integration.sh` (run via `nix develop .#integration --command bash scripts/test-integration.sh`)

Tests:
1. ✅ Discovery — `GET /discover` returns valid manifest (protocolMode, service names, EXCLUSIVE handler for virtual objects)
2. ✅ Direct echo — `POST /Echo/echo` with binary framing returns input bytes
3. ✅ Restate server starts and accepts registration
4. ✅ Greeter first call — returns greeting with count = 1
5. ✅ Greeter second call — state persisted, count = 2
6. ✅ Admin API confirms Greeter service registration

**Stage 5 — Package and publish ✅**
- API docs written in UCM (serve, Ctx, State, HandlerDef, ServiceDef) and pushed to Unison Share
- `README.md` written for the GitHub repo (installation, quick example, concepts, integration test instructions)
- Pushed to Unison Share as `@guillaumedesforges/restate-sdk-unison`
- URL: https://share.unison-lang.org/@guillaumedesforges/restate-sdk-unison
- GitHub repo: https://github.com/GuillaumeDesforges/restate-sdk-unison
- CI workflow: builds and symbol-checks the native library on every push
- Release workflow: builds prebuilt binaries (Linux x86_64, macOS ARM) and uploads to GitHub Releases on `v*` tags
- v0.1.0 tagged and released

## Key Unison syntax discoveries

- **Ability handlers**: `handle expr with f` where `f : Request {A, B} r ->{IO, Exception} r` — the handler is a named `cases` function, NOT inline cases. The `{ op args -> k }` brace syntax is used in arms; `handle k result with f` resumes the continuation.
- **Handler effects**: Without an explicit `Request` type annotation on `go`, Unison infers it as pure and rejects IO in arm bodies.
- **Reserved keyword**: `handle` cannot be a record field name — use `rawHandle`.
- **Lambda bindings**: `=` bindings inside lambda arguments cause parse errors; inline the expression.
- **Record constructors**: `type Foo.Bar = { x : Int }` creates constructor `Foo.Bar.Bar`.

---

## Build log — what was done and what went wrong

### Step 1 — Project scaffold

Created `CLAUDE.md`, `PROJECT.md`, `flake.nix`, `.mcp.json`, and `.claude/settings.json`. The dev shell declares `unison-ucm`, `cargo`, `rustc`, and `gcc` as build inputs, and the `shellHook` builds the Rust cdylib and exports `LD_LIBRARY_PATH` so UCM can find it via `openByName`. No code challenges at this step.

---

### Step 2 — Rust FFI wrapper (`crates/restate-sdk-unison-native/src/lib.rs`)

**What was done.** Wrote ~30 `#[no_mangle] extern "C"` functions wrapping `CoreVM` from `restate-sdk-shared-core`: lifecycle (`vm_new`, `vm_free`), input (`notify_input`, `notify_input_closed`, `notify_error`), output (`take_output`, `get_response_head`), syscalls (`sys_input`, `sys_state_get/set/clear/clear_all/get_keys`, `sys_sleep`, `sys_call`, `sys_send`, `sys_awakeable`, `sys_complete_awakeable`, `sys_run`, `propose_run_completion`, `sys_write_output`, `sys_end`), and the progress loop (`do_await`, `take_notification`). Built as `crate-type = ["cdylib"]`.

**Challenge: no null-pointer check in Unison.** The first design used `*mut c_void` as the VM handle — the standard C idiom. Allocation failure returns null, and callers check `ptr == NULL`. Unison's FFI exposes no equivalent of `Ptr.isNull`, so there was no way to detect failure. Redesigned the entire ABI: every handle is a `u64` cast from a heap pointer (`Box::into_raw(h) as u64`). `0` signals failure. This required touching every function signature on both sides.

**Challenge: complex return types across the FFI boundary.** Returning a struct like `ResponseHead { status: u16, headers: Vec<(String, String)> }` across C requires either a fixed-layout struct (fragile) or a separately allocated buffer. Chose instead to JSON-encode all complex return values on the Rust side and write the JSON bytes into a caller-supplied pinned buffer. Unison decodes with its JSON library. This meant every complex return type needed a custom serializer on the Rust side and a corresponding `Decoder` on the Unison side.

---

### Step 3 — Core types (`scratch/01_types.u`)

**What was done.** Defined `Restate.Serde`, `Restate.Input`, `Restate.Target`, `Restate.TerminalFailure`, `Restate.Future`, `Restate.AwaitResult`, `Restate.NotifValue`, `Restate.RunHandle`, `Restate.AwakeableHandle`, `Restate.CallHandles`, `Restate.ResponseHead`.

**Challenge: `handle` is a reserved keyword.** `type Restate.RunHandle = { handle : Nat }` compiled without a parse error but produced confusing downstream failures. Renamed the field to `rawHandle`.

**Challenge: double-qualified constructors.** Unison record types generate a constructor whose name is the type name repeated. `type Restate.AwakeableHandle = { id : Text, rawHandle : Nat }` creates constructor `Restate.AwakeableHandle.AwakeableHandle`, not `Restate.AwakeableHandle`. Every site that constructed one of these values had to use the double-qualified form.

---

### Step 4 — Native FFI bindings (`scratch/02_native.u`)

**What was done.** 47 definitions: utility wrappers (`withOutBuf`, `withBytesIn`, `withTextIn`, `checkVoid`, `checkHandle`), JSON decoders for all complex return types, and one Unison function per C export. Each function loads the DLL and symbol fresh on every call (no global state needed).

**Challenge: `=` bindings inside lambda arguments.** Unison's parser rejects assignment statements inside a lambda that is passed as a function argument. Writing `f (pin -> n = g pin; decode n)` produces "surprised to find `=` here". The fix was to always inline: `f (pin -> decode (g pin))`.

**Challenge: inline `match` inside list or tuple context.** Writing `("key", match x with None -> Null; Some k -> Text k)` failed because `;` inside a tuple literal is parsed as a list separator, not a match-arm separator. Extracted helper functions like `optTextJson : Optional Text -> Json` that perform the match as a standalone expression.

**Challenge: recursion without a dummy argument.** A helper defined as `go = match ... with ... -> go` was rejected — a zero-argument thunk with no effects cannot refer to itself recursively in a way the typechecker accepts. The fix: `go _ = match ...` with `go ()` at the call site. The dummy `()` argument makes `go` a genuine function rather than a thunk.

**Challenge: multi-line `match` with `;` separators.** Some matches written on a single line (`match x with A -> 1; B -> 2`) were misparsed as two separate block statements. All matches were written in multi-line form with indented arms.

---

### Step 5 — Ability layer (`scratch/03_ability.u`)

This was the hardest step. Three distinct failure modes before a working pattern was found.

**What was done.** Defined `Restate.Ctx` and `Restate.State` abilities. Wrote the progress loop `Restate.Vm.awaitHandle`, arm helpers (`hdlSleep`, `hdlSend`, `hdlRun`, `hdlStateGet`, `hdlStateKeys`), and the main interpreter `Restate.Vm.runHandler`.

**Failure 1: wrong handler syntax.** The natural-looking form

```
Restate.Vm.runHandler vm handler = handle !handler with
  Restate.Ctx.ctx.input k -> k (Restate.Vm.sysInput vm)
  ...
```

produced: "I found an action in a block with a type of `ctx.input -> (Input ->{𝕖} 𝕩) ->{𝕖, IO, Exception} 𝕩`". The typechecker was reading the `with` block as a sequence of standalone expressions in the function body, not as handler arms. Multiple reformulations (nested `handle` expressions, splitting Ctx and State into separate handlers) produced the same error.

**Discovery: the correct pattern.** Inspecting `lib.unison_json_1_4_2.Decoder.tryRunParsed` in the JSON library source revealed the actual Unison idiom:

```
go = cases
  { a }             -> Right a
  { value! -> k }   -> handle k j with go
  { failWith e -> _ } -> Left e
handle d() with go
```

The `with` clause takes a **named function** defined with `cases`, not an inline block. Each arm uses `{ operation args -> k }` brace syntax, and the continuation is resumed with `handle k result with go`.

**Failure 2: handler inferred as pure.** After rewriting with the correct syntax, the typechecker reported: "The expression in red needs the `{IO}` ability, but this location does not have access to any abilities." The named `go = cases ...` function was being inferred as pure (no effects in its return type), causing every IO call in the arm bodies to be rejected. The problem: without a type annotation, Unison has no signal that `go`'s arms should be allowed to perform IO.

**Fix: explicit `Request` type annotation.** The `Request` builtin (`lib.unison_base_7_19_2.abilities.Request`) is the type of the value passed to an ability handler. Annotating `go` explicitly:

```
go : Request {Restate.Ctx, Restate.State} Bytes ->{IO, Exception} Bytes
go = cases
  ...
```

told the typechecker that the arms return `{IO, Exception} Bytes`, making IO calls legal. The file typechecked immediately after adding this annotation.

---

### Step 6 — HTTP endpoint (`scratch/04_endpoint.u`)

**What was done.** Defined `Restate.ServiceKind`, `Restate.HandlerDef`, `Restate.ServiceDef`. Wrote `encodeDiscovery` (JSON discovery response), `flatHeaders` (convert `Headers` to `[(Text, Text)]` for the VM), `collectOutput` (drain VM output bytes after handler completes), `runInvocation` (full VM lifecycle per request), and `Restate.Endpoint.serve` (HTTP routing with `unison_http_16_1_0`).

**Challenge: learning the HTTP library API.** The library's routing model (`Handler.Handler cases req | predicate req -> response; _ -> abort`) and path representation (`Path { segments : [Text] }`) required reading the library source. Path parsing uses `match HttpRequest.uri req with URI _ _ path _ _ -> Path.segments path` to get `[svcName, hdlName]`.

**Challenge: `Json.Number` takes `Text`.** The JSON library stores numbers unparsed — `Json.Number : Text -> Json`, not `Json.Number : Float -> Json`. The discovery response uses `Json.nat` (a helper that converts `Nat` to `Json.Number` via `Nat.toText`).

No type errors on first typecheck.

---

### Step 7 — Greeter example (`scratch/05_example.u`)

**What was done.** Two example services:

- `Restate.Example.Echo` — stateless service with `echoFn` that returns its input bytes unchanged. `Restate.Example.mainEcho` serves it on port 9080. Used as the direct-echo target in Stage 4 integration tests.
- `Restate.Example.Greeter` — virtual object handler (`greet`) that reads the caller's name from `ctx.input`, reads and increments a per-key counter via `state.get`/`state.set`, and returns a UTF-8 greeting. `Restate.Example.main` serves it on port 9080.

No challenges. The patterns from earlier layers transferred directly. Both typechecked first try.

---

### Step 8 — Stage 4 integration tests (`scripts/test-integration.sh`)

**What was done.** Wrote a shell script that: (1) starts `ucm transcript.in-place` as a background server, (2) polls until port 9080 is ready, (3) runs discovery and direct echo tests, (4) optionally starts `restate-server` and runs the full Restate registration + invocation flow. All 6 tests pass.

**Challenge: discovery content-type.** Restate 1.6.2 requires `content-type: application/vnd.restate.endpointmanifest.v4+json`. Our initial implementation returned `application/json`. Fix: updated `Restate.Endpoint.serve`'s discovery handler to use the versioned content-type.

**Challenge: `protocolMode` field.** Restate requires `"protocolMode": "REQUEST_RESPONSE"` in the manifest for HTTP/1.1 endpoints. Added to `encodeDiscovery`.

**Challenge: Restate uses `/invoke/{service}/{handler}` paths.** Restate does NOT send invocations to `/{service}/{handler}`. It sends to `/invoke/{service}/{handler}`. The key (for virtual objects) is embedded in the binary StartMessage frame body, not the URL. Fix: updated routing to match `["invoke", svcName, hdlName]` in addition to the direct `[svcName, hdlName]` form used in tests.

**Challenge: virtual object handlers must be `EXCLUSIVE`.** `SHARED` handlers cannot write state. Our `encodeDiscovery` was hardcoding `"ty": "SHARED"` for all handlers. Virtual object handlers that write state must be `"EXCLUSIVE"`. Fix: `encodeHdl` now derives the handler type from the service kind — `RsService` → `"SHARED"`, `RsVirtualObject` → `"EXCLUSIVE"`.

**Challenge: restate-server persists state in `~/.restate`.** When restarting `restate-server` between test runs, it was loading the old deployment registration (with the wrong `SHARED` handler type) from disk. Fix: added `--base-dir $(mktemp -d)` to use a fresh temporary directory per test run. The temp dir is cleaned up by the `cleanup` trap.

**Challenge: HTTP/2 vs HTTP/1.1.** Restate tries HTTP/2 for service registration by default. `unison_http_16_1_0` only speaks HTTP/1.1. Fix: added `"use_http_11": true` to the registration JSON payload.
