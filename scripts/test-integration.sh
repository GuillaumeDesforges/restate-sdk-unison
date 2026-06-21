#!/usr/bin/env bash
# Stage 4 integration tests.
#
# Usage:
#   nix develop .#default     --command bash scripts/test-integration.sh   # discovery + echo only
#   nix develop .#integration --command bash scripts/test-integration.sh   # + full Restate round-trip
#
# Must be run from the repo root inside a nix develop shell so that:
#   - ucm is in PATH
#   - LD_LIBRARY_PATH contains the Rust cdylib (set by shellHook)
#   - curl is in PATH

set -euo pipefail

SDK_PORT=9080
RESTATE_ADMIN=http://localhost:9070
RESTATE_INVOKE=http://localhost:8080
SDK_URL=http://localhost:${SDK_PORT}

UCM_PID=""
RESTATE_PID=""
TRANSCRIPT=""
INPUT_FILE=""
RESPONSE_FILE=""
RESTATE_DATA_DIR=""

cleanup() {
    set +e
    [ -n "$UCM_PID" ]          && kill "$UCM_PID"       2>/dev/null; wait "$UCM_PID"       2>/dev/null
    [ -n "$RESTATE_PID" ]      && kill "$RESTATE_PID"   2>/dev/null; wait "$RESTATE_PID"   2>/dev/null
    [ -n "$TRANSCRIPT" ]       && rm -f "$TRANSCRIPT"
    [ -n "$INPUT_FILE" ]       && rm -f "$INPUT_FILE"
    [ -n "$RESPONSE_FILE" ]    && rm -f "$RESPONSE_FILE"
    [ -n "$RESTATE_DATA_DIR" ] && rm -rf "$RESTATE_DATA_DIR"
}
trap cleanup EXIT

wait_for_port() {
    local url="$1" label="$2" attempts="${3:-30}"
    printf "Waiting for %s " "$label"
    for i in $(seq 1 "$attempts"); do
        if curl -sf "$url" > /dev/null 2>&1; then
            echo " ready."
            return 0
        fi
        sleep 1
        printf "."
    done
    echo " TIMEOUT"
    return 1
}

# ── Build frame bytes using Python ───────────────────────────────────────────

MAKE_FRAMES='
import sys

def enc_var(v):
    buf = []
    while True:
        lo = v & 0x7F; v >>= 7
        if v == 0: buf.append(lo); break
        buf.append(lo | 0x80)
    return bytes(buf)

def proto_bytes_field(field, data):
    return enc_var((field << 3) | 2) + enc_var(len(data)) + data

def proto_varint_field(field, val):
    return enc_var(field << 3) + enc_var(val)

def restate_frame(type_code, body):
    return ((type_code << 48) | len(body)).to_bytes(8, "big") + body

input_bytes = sys.stdin.buffer.read()
value_body  = proto_bytes_field(1, input_bytes)
input_body  = proto_bytes_field(14, value_body)
start_body  = proto_varint_field(3, 1)
frames = restate_frame(0x0000, start_body) + restate_frame(0x0400, input_body)
sys.stdout.buffer.write(frames)
'

EXTRACT_OUTPUT='
import sys

def decode_varint(buf, pos):
    val, shift = 0, 0
    while True:
        b = buf[pos]; pos += 1
        val |= (b & 0x7F) << shift; shift += 7
        if not (b & 0x80): return val, pos

data = sys.stdin.buffer.read()
pos = 0
while pos + 8 <= len(data):
    header    = int.from_bytes(data[pos:pos+8], "big")
    type_code = header >> 48
    body_len  = header & 0xFFFFFFFF
    body      = data[pos+8:pos+8+body_len]; pos += 8 + body_len
    if type_code != 0x0401:
        continue
    bpos = 0
    while bpos < len(body):
        tag, bpos = decode_varint(body, bpos)
        field, wire = tag >> 3, tag & 7
        if field == 14 and wire == 2:
            vlen, bpos = decode_varint(body, bpos)
            vbytes = body[bpos:bpos+vlen]
            vbpos = 0
            while vbpos < len(vbytes):
                vtag, vbpos = decode_varint(vbytes, vbpos)
                vf, vw = vtag >> 3, vtag & 7
                if vf == 1 and vw == 2:
                    clen, vbpos = decode_varint(vbytes, vbpos)
                    sys.stdout.buffer.write(vbytes[vbpos:vbpos+clen]); sys.exit(0)
            sys.exit(0)
        else:
            if wire == 0:
                _, bpos = decode_varint(body, bpos)
            elif wire == 2:
                l, bpos = decode_varint(body, bpos); bpos += l
'

# ── 1. Start the SDK endpoint ────────────────────────────────────────────────

TRANSCRIPT=$(mktemp /tmp/ucm-server-XXXXXX.md)
cat > "$TRANSCRIPT" << 'EOF'
```ucm
@gdforj/restate-sdk-unison/main> run Restate.Example.mainEcho
```
EOF

echo "Starting SDK endpoint on port $SDK_PORT..."
ucm transcript.in-place "$TRANSCRIPT" &
UCM_PID=$!
wait_for_port "$SDK_URL/discover" "SDK endpoint"

# ── 2. Discovery test ────────────────────────────────────────────────────────

echo ""
echo "=== Test 1: Discovery ==="
discover=$(curl -sf "$SDK_URL/discover")
echo "$discover" | jq .
echo "$discover" | jq -e '
  .minProtocolVersion == 5 and
  .maxProtocolVersion == 7 and
  .protocolMode == "REQUEST_RESPONSE" and
  ([.services[].name] | contains(["Echo", "Greeter"]))
' > /dev/null
echo "PASS: discovery JSON valid (minProtocol=5, maxProtocol=7, protocolMode=REQUEST_RESPONSE, services Echo+Greeter)"

# ── 3. Direct echo invocation (no Restate needed) ───────────────────────────

echo ""
echo "=== Test 2: Direct echo invocation ==="

INPUT_FILE=$(mktemp /tmp/echo-input-XXXXXX.bin)
RESPONSE_FILE=$(mktemp /tmp/echo-response-XXXXXX.bin)

# Build StartMessage + InputCommandMessage frames for "hello"
printf 'hello' | python3 -c "$MAKE_FRAMES" > "$INPUT_FILE"

curl -sf -X POST \
    -H "content-type: application/vnd.restate.invocation.v5" \
    --data-binary @"$INPUT_FILE" \
    "$SDK_URL/Echo/echo" \
    -o "$RESPONSE_FILE"

extracted=$(python3 -c "$EXTRACT_OUTPUT" < "$RESPONSE_FILE")

if [ "$extracted" = "hello" ]; then
    echo "PASS: echo output = 'hello'"
else
    echo "FAIL: expected 'hello', got '$extracted'"
    exit 1
fi

# ── 4. Full Restate integration (requires nix develop .#integration) ─────────

if ! command -v restate-server > /dev/null 2>&1; then
    echo ""
    echo "=== Tests 3-6: Restate integration SKIPPED ==="
    echo "  (restate-server not in PATH)"
    echo "  Run with: nix develop .#integration --command bash scripts/test-integration.sh"
    echo ""
    echo "All available tests passed!"
    exit 0
fi

echo ""
echo "=== Test 3: Restate integration — starting restate-server ==="
RESTATE_DATA_DIR=$(mktemp -d /tmp/restate-data-XXXXXX)
restate-server --base-dir "$RESTATE_DATA_DIR" &
RESTATE_PID=$!
wait_for_port "$RESTATE_ADMIN/health" "restate admin"
sleep 2  # wait for invoke port

# Register
echo "Registering SDK endpoint..."
reg=$(curl -sf -X POST "$RESTATE_ADMIN/deployments" \
    -H "content-type: application/json" \
    -d "{\"uri\": \"$SDK_URL\", \"use_http_11\": true}")
echo "$reg" | jq .
echo "Registered."

echo ""
echo "=== Test 4: Greeter first call (count = 1) ==="
resp1=$(curl -sf -X POST "$RESTATE_INVOKE/Greeter/alice/greet" \
    -H "content-type: application/octet-stream" \
    --data-binary "Alice")
echo "Response: $resp1"
echo "$resp1" | grep -q "Alice" || { echo "FAIL: missing 'Alice'"; exit 1; }
echo "$resp1" | grep -q "1"     || { echo "FAIL: missing count 1"; exit 1; }
echo "PASS: first Greeter call succeeded"

echo ""
echo "=== Test 5: Greeter second call (count = 2) ==="
resp2=$(curl -sf -X POST "$RESTATE_INVOKE/Greeter/alice/greet" \
    -H "content-type: application/octet-stream" \
    --data-binary "Alice")
echo "Response: $resp2"
echo "$resp2" | grep -q "2" || { echo "FAIL: missing count 2"; exit 1; }
echo "PASS: state persisted across calls (count = 2)"

echo ""
echo "=== Test 6: Restate admin accepted service registration ==="
svc=$(curl -sf "$RESTATE_ADMIN/services/Greeter")
echo "$svc" | jq .
echo "$svc" | jq -e '.name == "Greeter"' > /dev/null
echo "PASS: Restate admin API confirmed service Greeter"

echo ""
echo "All tests passed!"
