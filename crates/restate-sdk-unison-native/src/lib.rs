use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use bytes::Bytes;
use restate_sdk_shared_core::{
    AwaitResponse, CoreVM, Error as VmError, NonEmptyValue, PayloadOptions, RetryPolicy,
    RunExitResult, State, Target, TerminalFailure, UnresolvedFuture, Value, VMOptions, VM,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ── Opaque VM handle ────────────────────────────────────────────────────────

struct VMHandle {
    vm: CoreVM,
    overflow: Bytes,
}

// Encode a Box<VMHandle> as a u64 integer handle (0 = null/error).
fn to_handle(h: Box<VMHandle>) -> u64 {
    Box::into_raw(h) as u64
}

unsafe fn from_handle(h: u64) -> &'static mut VMHandle {
    &mut *(h as *mut VMHandle)
}

unsafe fn drop_handle(h: u64) {
    drop(Box::from_raw(h as *mut VMHandle));
}

// ── JSON types for the C boundary ───────────────────────────────────────────

#[derive(Deserialize)]
struct JsonHeaders(Vec<[String; 2]>);

struct HeaderMapImpl(Vec<(String, String)>);

impl restate_sdk_shared_core::HeaderMap for HeaderMapImpl {
    type Error = std::convert::Infallible;
    fn extract(&self, name: &str) -> Result<Option<&str>, Self::Error> {
        for (k, v) in &self.0 {
            if k.eq_ignore_ascii_case(name) {
                return Ok(Some(v.as_str()));
            }
        }
        Ok(None)
    }
}

#[derive(Serialize)]
struct JsonResponseHead {
    status: u16,
    headers: Vec<[String; 2]>,
}

#[derive(Deserialize)]
#[serde(tag = "tag")]
enum JsonFuture {
    Single { handle: u32 },
    FirstCompleted { children: Vec<JsonFuture> },
    AllCompleted { children: Vec<JsonFuture> },
    FirstSucceededOrAllFailed { children: Vec<JsonFuture> },
    AllSucceededOrFirstFailed { children: Vec<JsonFuture> },
    Unknown { children: Vec<JsonFuture> },
}

#[derive(Serialize)]
#[serde(tag = "tag")]
#[allow(non_snake_case)]
enum JsonAwaitResponse {
    AnyCompleted,
    WaitingExternalProgress { waitingInput: bool, waitingRunProposal: bool },
    ExecuteRun { handle: u32 },
    CancelSignalReceived,
}

#[derive(Serialize)]
#[serde(tag = "tag")]
enum JsonValue {
    Void,
    Success { data: String },
    Failure { code: u16, message: String },
    StateKeys { keys: Vec<String> },
    InvocationId { id: String },
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct JsonInput {
    invocationId: String,
    randomSeed: u64,
    key: String,
    headers: Vec<[String; 2]>,
    input: String,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct JsonTarget {
    service: String,
    handler: String,
    key: Option<String>,
    idempotencyKey: Option<String>,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct JsonCallHandle {
    invocIdHandle: u32,
    callHandle: u32,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct JsonSendHandle {
    invocIdHandle: u32,
}

#[derive(Serialize)]
struct JsonAwakeableHandle {
    id: String,
    handle: u32,
}

#[derive(Serialize)]
struct JsonRunHandle {
    replayed: bool,
    handle: u32,
}

#[derive(Deserialize)]
struct JsonFailure {
    code: u16,
    message: String,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn write_json<T: Serialize>(val: &T, buf: *mut u8, cap: usize) -> i64 {
    match serde_json::to_vec(val) {
        Err(_) => -1,
        Ok(bytes) => {
            if bytes.len() > cap {
                return -2;
            }
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len()) };
            bytes.len() as i64
        }
    }
}

unsafe fn bytes_from_raw(ptr: *const u8, len: usize) -> Bytes {
    if len == 0 || ptr.is_null() {
        return Bytes::new();
    }
    Bytes::copy_from_slice(std::slice::from_raw_parts(ptr, len))
}

unsafe fn str_from_raw<'a>(ptr: *const u8, len: usize) -> &'a str {
    std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len))
}

unsafe fn json_from_raw<T: for<'de> Deserialize<'de>>(ptr: *const u8, len: usize) -> Option<T> {
    serde_json::from_slice(std::slice::from_raw_parts(ptr, len)).ok()
}

fn convert_future(f: JsonFuture) -> UnresolvedFuture {
    match f {
        JsonFuture::Single { handle } => UnresolvedFuture::Single(handle.into()),
        JsonFuture::FirstCompleted { children } => {
            UnresolvedFuture::FirstCompleted(children.into_iter().map(convert_future).collect())
        }
        JsonFuture::AllCompleted { children } => {
            UnresolvedFuture::AllCompleted(children.into_iter().map(convert_future).collect())
        }
        JsonFuture::FirstSucceededOrAllFailed { children } => {
            UnresolvedFuture::FirstSucceededOrAllFailed(
                children.into_iter().map(convert_future).collect(),
            )
        }
        JsonFuture::AllSucceededOrFirstFailed { children } => {
            UnresolvedFuture::AllSucceededOrFirstFailed(
                children.into_iter().map(convert_future).collect(),
            )
        }
        JsonFuture::Unknown { children } => {
            UnresolvedFuture::Unknown(children.into_iter().map(convert_future).collect())
        }
    }
}

fn convert_value(v: Value) -> JsonValue {
    match v {
        Value::Void => JsonValue::Void,
        Value::Success(b) => JsonValue::Success { data: BASE64.encode(&b) },
        Value::Failure(f) => JsonValue::Failure { code: f.code, message: f.message },
        Value::StateKeys(keys) => JsonValue::StateKeys { keys },
        Value::InvocationId(id) => JsonValue::InvocationId { id },
    }
}

fn json_failure_to_terminal(bytes: Bytes) -> Option<TerminalFailure> {
    let f: JsonFailure = serde_json::from_slice(&bytes).ok()?;
    Some(TerminalFailure { code: f.code, message: f.message, metadata: vec![] })
}

// ── C ABI — integer handle convention ────────────────────────────────────────
// All functions take/return u64 handles (0 = null/error for vm_new,
// -1 returned as i64 for errors in all other functions).

/// Create a VM from JSON-encoded request headers `[[name,value],...]`.
/// Returns a non-zero u64 handle on success, 0 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_new(headers_json: *const u8, len: usize) -> u64 {
    let headers: JsonHeaders =
        match serde_json::from_slice(std::slice::from_raw_parts(headers_json, len)) {
            Ok(h) => h,
            Err(_) => return 0,
        };
    let header_map = HeaderMapImpl(headers.0.into_iter().map(|[k, v]| (k, v)).collect());
    match CoreVM::new(header_map, VMOptions::default()) {
        Ok(vm) => to_handle(Box::new(VMHandle { vm, overflow: Bytes::new() })),
        Err(_) => 0,
    }
}

/// Free a VM handle created by `restate_vm_new`.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_free(vm: u64) {
    if vm != 0 {
        drop_handle(vm);
    }
}

/// Write the HTTP response head as JSON into `buf`.
/// Returns bytes written, or -1 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_get_response_head(
    vm: u64,
    buf: *mut u8,
    cap: usize,
) -> i64 {
    let h = &from_handle(vm).vm;
    let head = h.get_response_head();
    let out = JsonResponseHead {
        status: head.status_code,
        headers: head
            .headers
            .iter()
            .map(|h| [h.key.to_string(), h.value.to_string()])
            .collect(),
    };
    write_json(&out, buf, cap)
}

/// Feed input bytes to the VM.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_notify_input(vm: u64, buf: *const u8, len: usize) {
    let h = &mut from_handle(vm).vm;
    h.notify_input(bytes_from_raw(buf, len));
}

/// Signal end of the input stream.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_notify_input_closed(vm: u64) {
    let h = &mut from_handle(vm).vm;
    h.notify_input_closed();
}

/// Report a non-terminal error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_notify_error(
    vm: u64,
    msg: *const u8,
    msg_len: usize,
    code: u16,
) {
    let h = &mut from_handle(vm).vm;
    let text = str_from_raw(msg, msg_len).to_owned();
    h.notify_error(VmError::new(code, text), None);
}

/// Pull output bytes from the VM into `buf`.
/// Returns bytes written (>= 0), or -1 when the VM is closed and fully drained.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_take_output(vm: u64, buf: *mut u8, cap: usize) -> i64 {
    let handle = from_handle(vm);

    if !handle.overflow.is_empty() {
        let n = handle.overflow.len().min(cap);
        std::ptr::copy_nonoverlapping(handle.overflow.as_ptr(), buf, n);
        handle.overflow = handle.overflow.slice(n..);
        return n as i64;
    }

    let output = handle.vm.take_output();
    if output.is_empty() {
        return if handle.vm.state() == State::Closed { -1 } else { 0 };
    }

    let n = output.len().min(cap);
    std::ptr::copy_nonoverlapping(output.as_ptr(), buf, n);
    if output.len() > cap {
        handle.overflow = output.slice(cap..);
    }
    n as i64
}

/// Returns 1 if ready to execute, 0 if not, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_is_ready_to_execute(vm: u64) -> i32 {
    let h = &from_handle(vm).vm;
    match h.is_ready_to_execute() {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => -1,
    }
}

/// Returns VM state: 0=WaitingPreFlight, 1=Replaying, 2=Processing, 3=Closed.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_state(vm: u64) -> i32 {
    let h = &from_handle(vm).vm;
    match h.state() {
        State::WaitingPreFlight => 0,
        State::Replaying => 1,
        State::Processing => 2,
        State::Closed => 3,
    }
}

/// Returns 1 if the notification handle is completed, 0 if not.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_is_completed(vm: u64, handle: u32) -> i32 {
    let h = &from_handle(vm).vm;
    if h.is_completed(handle.into()) { 1 } else { 0 }
}

/// Returns the last command index, or -1 if no command yet.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_last_command_index(vm: u64) -> i64 {
    let h = &from_handle(vm).vm;
    h.last_command_index()
}

/// Drive the progress loop with an `UnresolvedFuture` encoded as JSON.
/// Writes a JSON `AwaitResponse` into `result_buf`. Returns bytes written or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_do_await(
    vm: u64,
    future_json: *const u8,
    future_len: usize,
    result_buf: *mut u8,
    result_cap: usize,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    let jf: JsonFuture = match json_from_raw(future_json, future_len) {
        Some(f) => f,
        None => return -1,
    };
    let future = convert_future(jf);
    let resp = match h.do_await(future) {
        Ok(r) => r,
        Err(_) => return -1,
    };
    let jr = match resp {
        AwaitResponse::AnyCompleted => JsonAwaitResponse::AnyCompleted,
        AwaitResponse::WaitingExternalProgress { waiting_input, waiting_run_proposal } => {
            JsonAwaitResponse::WaitingExternalProgress {
                waitingInput: waiting_input,
                waitingRunProposal: waiting_run_proposal,
            }
        }
        AwaitResponse::ExecuteRun(nh) => JsonAwaitResponse::ExecuteRun { handle: u32::from(nh) },
        AwaitResponse::CancelSignalReceived => JsonAwaitResponse::CancelSignalReceived,
    };
    write_json(&jr, result_buf, result_cap)
}

/// Take a completed notification. Writes a JSON `Value` into `result_buf`.
/// Returns bytes written or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_take_notification(
    vm: u64,
    handle: u32,
    result_buf: *mut u8,
    result_cap: usize,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    match h.take_notification(handle.into()) {
        Ok(Some(v)) => write_json(&convert_value(v), result_buf, result_cap),
        Ok(None) => write_json(&JsonValue::Void, result_buf, result_cap),
        Err(_) => -1,
    }
}

/// Retrieve the invocation input as JSON. Returns bytes written or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_input(
    vm: u64,
    buf: *mut u8,
    cap: usize,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    match h.sys_input() {
        Ok(inp) => {
            let out = JsonInput {
                invocationId: inp.invocation_id,
                randomSeed: inp.random_seed,
                key: inp.key,
                headers: inp
                    .headers
                    .iter()
                    .map(|hdr| [hdr.key.to_string(), hdr.value.to_string()])
                    .collect(),
                input: BASE64.encode(&inp.input),
            };
            write_json(&out, buf, cap)
        }
        Err(_) => -1,
    }
}

/// Read a state entry. Returns notification handle (>= 0) or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_state_get(
    vm: u64,
    key: *const u8,
    key_len: usize,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    match h.sys_state_get(str_from_raw(key, key_len).to_owned(), PayloadOptions::default()) {
        Ok(nh) => u32::from(nh) as i64,
        Err(_) => -1,
    }
}

/// List state keys. Returns notification handle (>= 0) or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_state_get_keys(vm: u64) -> i64 {
    let h = &mut from_handle(vm).vm;
    match h.sys_state_get_keys() {
        Ok(nh) => u32::from(nh) as i64,
        Err(_) => -1,
    }
}

/// Write a state entry. Returns 0 on success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_state_set(
    vm: u64,
    key: *const u8,
    key_len: usize,
    val: *const u8,
    val_len: usize,
) -> i32 {
    let h = &mut from_handle(vm).vm;
    match h.sys_state_set(
        str_from_raw(key, key_len).to_owned(),
        bytes_from_raw(val, val_len),
        PayloadOptions::default(),
    ) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Clear a state entry. Returns 0 on success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_state_clear(
    vm: u64,
    key: *const u8,
    key_len: usize,
) -> i32 {
    let h = &mut from_handle(vm).vm;
    match h.sys_state_clear(str_from_raw(key, key_len).to_owned()) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Clear all state entries. Returns 0 on success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_state_clear_all(vm: u64) -> i32 {
    let h = &mut from_handle(vm).vm;
    match h.sys_state_clear_all() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Schedule a sleep. `wake_up_ms` milliseconds since Unix epoch.
/// Returns notification handle (>= 0) or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_sleep(
    vm: u64,
    name: *const u8,
    name_len: usize,
    wake_up_ms: u64,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    match h.sys_sleep(
        str_from_raw(name, name_len).to_owned(),
        Duration::from_millis(wake_up_ms),
        None,
    ) {
        Ok(nh) => u32::from(nh) as i64,
        Err(_) => -1,
    }
}

/// Call another service. Returns JSON `{"invocIdHandle":N,"callHandle":M}` or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_call(
    vm: u64,
    target_json: *const u8,
    target_len: usize,
    input: *const u8,
    input_len: usize,
    result_buf: *mut u8,
    result_cap: usize,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    let jt: JsonTarget = match json_from_raw(target_json, target_len) {
        Some(t) => t,
        None => return -1,
    };
    let target = Target {
        service: jt.service,
        handler: jt.handler,
        key: jt.key,
        idempotency_key: jt.idempotencyKey,
        scope: None,
        limit_key: None,
        headers: vec![],
    };
    match h.sys_call(target, bytes_from_raw(input, input_len), None, PayloadOptions::default()) {
        Ok(ch) => write_json(
            &JsonCallHandle {
                invocIdHandle: u32::from(ch.invocation_id_notification_handle),
                callHandle: u32::from(ch.call_notification_handle),
            },
            result_buf,
            result_cap,
        ),
        Err(_) => -1,
    }
}

/// One-way call. `delay_ms` = 0 for immediate. Returns JSON `{"invocIdHandle":N}` or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_send(
    vm: u64,
    target_json: *const u8,
    target_len: usize,
    input: *const u8,
    input_len: usize,
    delay_ms: u64,
    result_buf: *mut u8,
    result_cap: usize,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    let jt: JsonTarget = match json_from_raw(target_json, target_len) {
        Some(t) => t,
        None => return -1,
    };
    let target = Target {
        service: jt.service,
        handler: jt.handler,
        key: jt.key,
        idempotency_key: jt.idempotencyKey,
        scope: None,
        limit_key: None,
        headers: vec![],
    };
    let exec_time = if delay_ms == 0 { None } else { Some(Duration::from_millis(delay_ms)) };
    match h.sys_send(
        target,
        bytes_from_raw(input, input_len),
        exec_time,
        None,
        PayloadOptions::default(),
    ) {
        Ok(sh) => write_json(
            &JsonSendHandle { invocIdHandle: u32::from(sh.invocation_id_notification_handle) },
            result_buf,
            result_cap,
        ),
        Err(_) => -1,
    }
}

/// Create an awakeable. Writes JSON `{"id":"...","handle":N}`. Returns bytes written or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_awakeable(vm: u64, buf: *mut u8, cap: usize) -> i64 {
    let h = &mut from_handle(vm).vm;
    match h.sys_awakeable() {
        Ok(aw) => write_json(
            &JsonAwakeableHandle { id: aw.id, handle: u32::from(aw.handle) },
            buf,
            cap,
        ),
        Err(_) => -1,
    }
}

/// Complete an awakeable. `is_success=1` → bytes; `0` → `{"code":N,"message":"..."}`.
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_complete_awakeable(
    vm: u64,
    id: *const u8,
    id_len: usize,
    val: *const u8,
    val_len: usize,
    is_success: i32,
) -> i32 {
    let h = &mut from_handle(vm).vm;
    let id_str = str_from_raw(id, id_len).to_owned();
    let val_bytes = bytes_from_raw(val, val_len);
    let nev = if is_success != 0 {
        NonEmptyValue::Success(val_bytes)
    } else {
        match json_failure_to_terminal(val_bytes) {
            Some(tf) => NonEmptyValue::Failure(tf),
            None => return -1,
        }
    };
    match h.sys_complete_awakeable(id_str, nev, PayloadOptions::default()) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Get a durable promise. Returns notification handle (>= 0) or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_get_promise(
    vm: u64,
    key: *const u8,
    key_len: usize,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    match h.sys_get_promise(str_from_raw(key, key_len).to_owned()) {
        Ok(nh) => u32::from(nh) as i64,
        Err(_) => -1,
    }
}

/// Complete a durable promise. Returns JSON `{"handle":N}` or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_complete_promise(
    vm: u64,
    key: *const u8,
    key_len: usize,
    val: *const u8,
    val_len: usize,
    is_success: i32,
    result_buf: *mut u8,
    result_cap: usize,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    let key_str = str_from_raw(key, key_len).to_owned();
    let val_bytes = bytes_from_raw(val, val_len);
    let nev = if is_success != 0 {
        NonEmptyValue::Success(val_bytes)
    } else {
        match json_failure_to_terminal(val_bytes) {
            Some(tf) => NonEmptyValue::Failure(tf),
            None => return -1,
        }
    };
    match h.sys_complete_promise(key_str, nev, PayloadOptions::default()) {
        Ok(nh) => write_json(
            &serde_json::json!({ "handle": u32::from(nh) }),
            result_buf,
            result_cap,
        ),
        Err(_) => -1,
    }
}

/// Begin a run closure. Writes JSON `{"replayed":bool,"handle":N}`. Returns bytes written or -1.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_run(
    vm: u64,
    name: *const u8,
    name_len: usize,
    buf: *mut u8,
    cap: usize,
) -> i64 {
    let h = &mut from_handle(vm).vm;
    match h.sys_run(str_from_raw(name, name_len).to_owned()) {
        Ok(rh) => write_json(
            &JsonRunHandle { replayed: rh.replayed, handle: u32::from(rh.handle) },
            buf,
            cap,
        ),
        Err(_) => -1,
    }
}

/// Propose completion of a run closure.
/// `is_success=1` → success bytes; `0` → `{"code":N,"message":"..."}`.
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_propose_run_completion(
    vm: u64,
    handle: u32,
    val: *const u8,
    val_len: usize,
    is_success: i32,
) -> i32 {
    let h = &mut from_handle(vm).vm;
    let val_bytes = bytes_from_raw(val, val_len);
    let result = if is_success != 0 {
        RunExitResult::Success(val_bytes)
    } else {
        match json_failure_to_terminal(val_bytes) {
            Some(tf) => RunExitResult::TerminalFailure(tf),
            None => return -1,
        }
    };
    match h.propose_run_completion(handle.into(), result, RetryPolicy::None) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Write the invocation output.
/// `is_success=1` → success bytes; `0` → `{"code":N,"message":"..."}`.
/// Returns 0 on success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_write_output(
    vm: u64,
    val: *const u8,
    val_len: usize,
    is_success: i32,
) -> i32 {
    let h = &mut from_handle(vm).vm;
    let val_bytes = bytes_from_raw(val, val_len);
    let nev = if is_success != 0 {
        NonEmptyValue::Success(val_bytes)
    } else {
        match json_failure_to_terminal(val_bytes) {
            Some(tf) => NonEmptyValue::Failure(tf),
            None => return -1,
        }
    };
    match h.sys_write_output(nev, PayloadOptions::default()) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// End the invocation. Returns 0 on success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn restate_vm_sys_end(vm: u64) -> i32 {
    let h = &mut from_handle(vm).vm;
    match h.sys_end() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

// ── Stage-3 test helpers ─────────────────────────────────────────────────────
// Build minimal Restate wire frames for unit tests and parse them back out.
// These are NOT part of the runtime ABI — only used by test programs.

fn encode_varint(mut val: u64, buf: &mut Vec<u8>) {
    loop {
        let lo = (val & 0x7F) as u8;
        val >>= 7;
        if val == 0 {
            buf.push(lo);
            return;
        }
        buf.push(lo | 0x80);
    }
}

fn proto_varint_field(field: u32, val: u64, buf: &mut Vec<u8>) {
    encode_varint((field as u64) << 3, buf); // wire type 0
    encode_varint(val, buf);
}

fn proto_bytes_field(field: u32, data: &[u8], buf: &mut Vec<u8>) {
    encode_varint(((field as u64) << 3) | 2, buf); // wire type 2
    encode_varint(data.len() as u64, buf);
    buf.extend_from_slice(data);
}

fn restate_wire_frame(type_code: u16, body: &[u8]) -> Vec<u8> {
    let header: u64 = ((type_code as u64) << 48) | (body.len() as u64);
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&header.to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// Build StartMessage (type 0x0000, known_entries=1) + InputCommandMessage (type 0x0400)
/// frames for the given input payload.
fn build_invocation_bytes(input: &[u8]) -> Vec<u8> {
    let mut start_body = Vec::new();
    proto_varint_field(3, 1, &mut start_body); // known_entries = 1

    let mut value_body = Vec::new();
    if !input.is_empty() {
        proto_bytes_field(1, input, &mut value_body); // Value.content
    }
    let mut input_body = Vec::new();
    proto_bytes_field(14, &value_body, &mut input_body); // InputCommandMessage.value

    let mut out = restate_wire_frame(0x0000, &start_body);
    out.extend(restate_wire_frame(0x0400, &input_body));
    out
}

/// Write StartMessage + InputCommandMessage frames for `input` into `out_buf`.
/// Returns bytes written (>= 0), or -2 if the buffer is too small.
#[no_mangle]
pub unsafe extern "C" fn restate_test_invocation_bytes(
    input_ptr: *const u8,
    input_len: usize,
    out_buf: *mut u8,
    out_cap: usize,
) -> i64 {
    let input = if input_len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(input_ptr, input_len)
    };
    let frames = build_invocation_bytes(input);
    if frames.len() > out_cap {
        return -2;
    }
    std::ptr::copy_nonoverlapping(frames.as_ptr(), out_buf, frames.len());
    frames.len() as i64
}

fn decode_varint_at(buf: &[u8], mut pos: usize) -> Option<(u64, usize)> {
    let mut val: u64 = 0;
    let mut shift = 0u32;
    loop {
        if pos >= buf.len() || shift > 63 {
            return None;
        }
        let b = buf[pos];
        pos += 1;
        val |= ((b & 0x7F) as u64) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            return Some((val, pos));
        }
    }
}

fn skip_proto_field(buf: &[u8], pos: usize, wire: u8) -> Option<usize> {
    match wire {
        0 => decode_varint_at(buf, pos).map(|(_, n)| n),
        1 => if pos + 8 <= buf.len() { Some(pos + 8) } else { None },
        2 => {
            let (len, n) = decode_varint_at(buf, pos)?;
            let end = n + len as usize;
            if end <= buf.len() { Some(end) } else { None }
        }
        5 => if pos + 4 <= buf.len() { Some(pos + 4) } else { None },
        _ => None,
    }
}

/// Extract Value.content bytes from the first OutputCommandMessage in `buf`.
fn extract_output_value(buf: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    while pos + 8 <= buf.len() {
        let header = u64::from_be_bytes(buf[pos..pos + 8].try_into().ok()?);
        let type_code = (header >> 48) as u16;
        let body_len = (header & 0xFFFF_FFFF) as usize;
        let body = &buf[pos + 8..pos + 8 + body_len.min(buf.len().saturating_sub(pos + 8))];
        if body.len() < body_len {
            return None;
        }
        pos += 8 + body_len;

        if type_code != 0x0401 {
            continue;
        }
        // Scan OutputCommandMessage for field 14 (Value, wire type 2)
        let mut bpos = 0;
        while bpos < body.len() {
            let (tag, after_tag) = decode_varint_at(body, bpos)?;
            let field = (tag >> 3) as u32;
            let wire = (tag & 7) as u8;
            bpos = after_tag;
            if field == 14 && wire == 2 {
                let (vlen, after_vlen) = decode_varint_at(body, bpos)?;
                let vlen = vlen as usize;
                let value_bytes = &body[after_vlen..after_vlen + vlen.min(body.len() - after_vlen)];
                if value_bytes.len() < vlen {
                    return None;
                }
                // Parse Value { bytes content = 1 }
                let mut vbpos = 0;
                while vbpos < value_bytes.len() {
                    let (vtag, after_vtag) = decode_varint_at(value_bytes, vbpos)?;
                    let vfield = (vtag >> 3) as u32;
                    let vwire = (vtag & 7) as u8;
                    vbpos = after_vtag;
                    if vfield == 1 && vwire == 2 {
                        let (clen, after_clen) = decode_varint_at(value_bytes, vbpos)?;
                        let clen = clen as usize;
                        if after_clen + clen > value_bytes.len() {
                            return None;
                        }
                        return Some(value_bytes[after_clen..after_clen + clen].to_vec());
                    }
                    vbpos = skip_proto_field(value_bytes, vbpos, vwire)?;
                }
                return Some(vec![]);
            }
            bpos = skip_proto_field(body, bpos, wire)?;
        }
    }
    None
}

/// Extract Value.content from the first OutputCommandMessage in `data`.
/// Returns bytes written (>= 0), -1 if not found, -2 if buffer too small.
#[no_mangle]
pub unsafe extern "C" fn restate_test_extract_output_value(
    data_ptr: *const u8,
    data_len: usize,
    out_buf: *mut u8,
    out_cap: usize,
) -> i64 {
    if data_len == 0 {
        return -1;
    }
    let data = std::slice::from_raw_parts(data_ptr, data_len);
    match extract_output_value(data) {
        None => -1,
        Some(content) => {
            if content.len() > out_cap {
                return -2;
            }
            std::ptr::copy_nonoverlapping(content.as_ptr(), out_buf, content.len());
            content.len() as i64
        }
    }
}
