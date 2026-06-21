# Restate SDK for Unison

[Restate](https://restate.dev/) is a system for building resilient applications using *distributed durable async/await*. This SDK enables [Unison](https://unison-lang.org/) programs to define durable handlers and run as Restate service endpoints.

Available on Unison Share: [`@guillaumedesforges/restate-sdk-unison`](https://share.unison-lang.org/@guillaumedesforges/restate-sdk-unison)

## Quick Example

```unison
greet : '{IO, Restate.State, Restate.Ctx, Exception} Bytes
greet = do
  inp = input
  name = Text.fromUtf8 (inputBytes inp)
  Text.toUtf8 ("Hello, " ++ name ++ "!")

greeter : Restate.ServiceDef
greeter = ServiceDef "Greeter" RsService [HandlerDef "greet" greet]

main : '{IO, Exception} ()
main = do
  Threads.run do
    Restate.Endpoint.serve 9080 [greeter]
    Threads.read Promise.new()
```

Run it, register it with Restate, and call it:

```bash
# Register
curl -X POST http://localhost:9070/deployments \
  -H 'content-type: application/json' \
  -d '{"uri": "http://localhost:9080", "use_http_11": true}'

# Invoke
curl -X POST http://localhost:8080/Greeter/greet \
  -H 'content-type: application/octet-stream' \
  --data-binary 'Alice'
```

## Getting Started

### 1. Install the library

In UCM:

```
lib.install @guillaumedesforges/restate-sdk-unison/main
```

### 2. Install the native library

The SDK wraps `restate-sdk-shared-core` via a Rust cdylib. Download the prebuilt binary for your platform from [GitHub Releases](https://github.com/GuillaumeDesforges/restate-sdk-unison/releases) and place it where your dynamic linker can find it:

| Platform       | File                                                       |
|----------------|------------------------------------------------------------|
| Linux x86_64   | `librestate_sdk_unison_native-x86_64-unknown-linux-gnu.so` |
| macOS ARM      | `librestate_sdk_unison_native-aarch64-apple-darwin.dylib`  |

```bash
# Linux example
export LD_LIBRARY_PATH=/path/to/lib:$LD_LIBRARY_PATH

# macOS example
export DYLD_LIBRARY_PATH=/path/to/lib:$DYLD_LIBRARY_PATH
```

Or build from source (requires Rust):

```bash
cargo build --release --manifest-path crates/restate-sdk-unison-native/Cargo.toml
```

### 3. Define and run a service

```unison
main : '{IO, Exception} ()
main = do
  Threads.run do
    Restate.Endpoint.serve 9080 [myService]
    Threads.read Promise.new()
```

```bash
ucm run MyProject.main
```

## Concepts

### Services

A stateless service with concurrent handlers:

```unison
echoHandler : '{IO, Restate.State, Restate.Ctx, Exception} Bytes
echoHandler = do
  inp = input
  inputBytes inp

echoService : Restate.ServiceDef
echoService = ServiceDef "Echo" RsService [HandlerDef "echo" echoHandler]
```

### Virtual Objects

A keyed, stateful service where each key has isolated state and handlers run exclusively per key:

```unison
counterHandler : '{IO, Restate.State, Restate.Ctx, Exception} Bytes
counterHandler = do
  countBs = state.get "count"
  count = match countBs with
    None    -> 0
    Some bs -> match Nat.fromText (Text.fromUtf8 bs) with
      None   -> 0
      Some n -> n
  newCount = count + 1
  state.set "count" (Text.toUtf8 (Nat.toText newCount))
  Text.toUtf8 (Nat.toText newCount)

counterObject : Restate.ServiceDef
counterObject = ServiceDef "Counter" RsVirtualObject [HandlerDef "increment" counterHandler]
```

### Abilities

Handler functions use two abilities:

- **`Restate.Ctx`** — read input, perform durable operations (sleep, call, send, run, awakeables)
- **`Restate.State`** — read and write per-key persistent state (get, set, clear, keys)

A handler thunk has type `'{IO, Restate.State, Restate.Ctx, Exception} Bytes` — it returns the raw response bytes.

### Serialization

The SDK uses explicit `Serde` records rather than a fixed serialization format:

```unison
type Restate.Serde a = { encode : a -> Bytes, decode : Bytes -> Either Text a }
```

Handlers receive and return raw `Bytes`. You choose the encoding (JSON, UTF-8 text, protobuf, etc.) by wrapping your logic accordingly.

## Running Integration Tests

Requirements: `nix develop` (see `flake.nix`).

```bash
# Start Restate server (terminal 1)
restate-server

# Start the SDK endpoint (terminal 2)
ucm run Restate.Example.main

# Run the test suite (terminal 3)
nix develop .#integration --command bash scripts/test-integration.sh
```

The script runs 6 tests: discovery, direct echo, Restate registration, Greeter call ×2, and admin API confirmation.

## Community & Resources

- **Documentation:** [docs.restate.dev](https://docs.restate.dev)
- **Discord:** [Join the community](https://discord.gg/skW3AZ6uGd)
- **Unison Share:** [@guillaumedesforges/restate-sdk-unison](https://share.unison-lang.org/@guillaumedesforges/restate-sdk-unison)
- **GitHub:** [GuillaumeDesforges/restate-sdk-unison](https://github.com/GuillaumeDesforges/restate-sdk-unison)
- **Issues:** [Report bugs or request features](https://github.com/GuillaumeDesforges/restate-sdk-unison/issues)
