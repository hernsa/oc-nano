# OC-Nano Rust Hot-Path Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a completely new, isolated Rust workspace `oc-nano` that implements all OpenCode memory-hot-path replacements in Rust via napi-rs, with NO naming/path/constant collision with the real OpenCode, and all 19 audit flaws fixed.

**Architecture:** Fresh workspace at `oc-nano/` with 7 crates (core, history, schema, state, prompt, compaction, napi) exposed as `oc_nano_native.dll`. All identifiers, paths, and constants renamed to avoid conflict with OpenCode (`oc-nano` binary, `oc-nano` config dir, `oc-nano` data dir, `oc-nano-history.db`). Fixes include: panic boundaries in every napi binding, `db_close`/`history_close` bindings, stable FxHash for cross-process schema hashing, streaming compaction without loading all messages into RAM, bounded RAM message cap enforcement, proper error messages, u64→i64 safe cast, streaming attachment reads, VACUUM only on explicit prune, TS↔Rust signature alignment, DLL path resolution from env var, version compatibility check.

**Tech Stack:** Rust 1.99.1 (GNU target), napi-rs 2.x, rusqlite 0.31 (bundled), fxhash, dashmap, lru, serde/serde_json, chrono, uuid, thiserror. Target: `x86_64-pc-windows-gnu` with MinGW-w64 + `rust-lld` linker.

**Spec:** `oc-nano/ARCHITECTURE.md` (new spec), fixes address all 19 audit findings from `opencode_memory_hot_paths_report.md`.

## Global Constraints

- Binary name must be `oc-nano` (never `opencode`, never `opencode-lean`) — total isolation from real OpenCode.
- DLL must be `oc_nano_native.dll` (never `opencode_lean_napi.dll`).
- Config dir: `~/.config/oc-nano/` (never `~/.config/opencode-lean/` or `~/.config/opencode/`).
- Data dir: `~/.local/share/oc-nano/` (never `~/.local/share/opencode-lean/`).
- History DB: `oc-nano-history.db` (never `history.sqlite`).
- Default RAM message cap: 2000 — ENFORCED (not just a constant).
- Default state TTL: 300 seconds.
- `rusqlite` must use `bundled` feature.
- `uuid` must use `serde` feature.
- `fxhash` for cross-process stable hashing (NOT DefaultHasher).
- All napi-exported numeric types must use `i64` (napi-rs does not support `u64`/`usize`).
- Every napi binding MUST have a panic boundary (catch_unwind) that returns a friendly napi error, never aborts the host process.
- `db_close`/`history_close` binding MUST exist to release DB handles.
- Build target: `x86_64-pc-windows-gnu` with MinGW-w64 + `rust-lld`.
- C drive has ~0.6-2 GB free — keep downloads minimal, `cargo clean` between major build steps.
- TS↔Rust signatures must match exactly (parameter count, types, order).
- TS wrapper must resolve DLL from `OC_NANO_DLL` env var, falling back to `rust/target/release/oc_nano_native.dll`.
- Version compatibility check binding must exist.

## Review Focus

| Failure mode | Expected behavior | Owning task |
|---|---|---|
| Panic in napi binding | Caught, returned as friendly napi error, host process survives | Task 1 (panic wrapper macro) |
| DB handle leak | `db_close` releases handle and closes connection | Task 3 (history close binding) |
| Schema cache misses after restart | FxHash stable across processes | Task 4 (schema cache) |
| Compaction OOMs on large history | Streaming compaction, never loads all messages | Task 6 (compaction) |
| TS↔Rust signature mismatch | Exact parameter/count/type alignment | All tasks (binding verification) |
| DLL not found from arbitrary cwd | Env var `OC_NANO_DLL` override + fallback | Task 8 (TS wrapper) |

---

### Task 1: Create oc-nano workspace scaffolding + panic-safe napi macro

**Files:**
- Create: `oc-nano/Cargo.toml` (root workspace)
- Create: `oc-nano/rust/Cargo.toml` (rust workspace)
- Create: `oc-nano/rust/.cargo/config.toml` (MinGW/gcc linker for gnu target)
- Create: `oc-nano/rust/oc-nano-macros/Cargo.toml`
- Create: `oc-nano/rust/oc-nano-macros/src/lib.rs` — `napi_panic_boundary!` macro wrapping every binding in `std::panic::catch_unwind`, converting panic to napi error
- Test: `oc-nano/rust/oc-nano-macros/tests/macro_test.rs`

**Interfaces:**
- Consumes: none (foundation)
- Produces: `napi_panic_boundary!($binding_block)` macro

- [ ] **Step 1: Write workspace Cargo.toml files**

Create root `oc-nano/Cargo.toml` and `oc-nano/rust/Cargo.toml` with `oc-nano-macros` as first member.

Run: verify TOML parses (cargo metadata)

Expected: valid workspace manifests

- [ ] **Step 2: Implement panic boundary macro**

Create `oc-nano-macros/src/lib.rs` with `#[macro_export] macro_rules! napi_panic_boundary` that wraps a block in `catch_unwind`, returns `Err(napi::Error::from_reason(format!("panic: {:?}", ...)))` on panic.

Run: `cargo test --package oc-nano-macros`

Expected: 1 test passes (panic caught and converted to error)

- [ ] **Step 3: Create .cargo/config.toml**

Point gnu target linker at MinGW gcc (or rust-lld).

Run: verify file exists

Expected: config.toml present

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: oc-nano workspace scaffolding + panic-safe napi macro"
```

---

### Task 2: oc-nano-core crate — types, constants, error model (completely renamed)

**Files:**
- Create: `oc-nano/rust/oc-nano-core/Cargo.toml`
- Create: `oc-nano/rust/oc-nano-core/src/lib.rs`
- Modify: `oc-nano/rust/Cargo.toml` — add `oc-nano-core` member
- Test: `oc-nano/rust/oc-nano-core/tests/core_test.rs`

**Interfaces:**
- Consumes: none
- Produces: `NanoId`, `SessionId`, `MsgRole`, `ToolInvocation`, `ToolOutcome`, `Message`, `ModelSettings`, `SchemaCacheEntry`, `NanoError`, `NanoResult<T>`, constants `BINARY_NAME="oc-nano"`, `CONFIG_DIR_NAME="oc-nano"`, `DATA_DIR_NAME="oc-nano"`, `HISTORY_DB_NAME="oc-nano-history.db"`, `DEFAULT_RAM_MESSAGE_CAP=2000`, `DEFAULT_STATE_TTL_SECS=300`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_constants_isolation() {
    assert_eq!(oc_nano_core::BINARY_NAME, "oc-nano");
    assert_ne!(oc_nano_core::BINARY_NAME, "opencode");
    assert_eq!(oc_nano_core::HISTORY_DB_NAME, "oc-nano-history.db");
    assert_ne!(oc_nano_core::HISTORY_DB_NAME, "history.sqlite");
}
```

Run: `cargo test --package oc-nano-core`

Expected: FAIL — `oc_nano_core` not defined

- [ ] **Step 2: Implement core types + constants**

All types renamed (no `opencode` prefix). `ModelSettings` must NOT derive `Eq` (has `Option<f64>`). `uuid` with `serde` feature.

Run: `cargo test --package oc-nano-core`

Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: oc-nano-core crate with renamed types and constants"
```

---

### Task 3: oc-nano-history crate — SQLite with close binding, bounded cache, streaming

**Files:**
- Create: `oc-nano/rust/oc-nano-history/Cargo.toml`
- Create: `oc-nano/rust/oc-nano-history/src/lib.rs`
- Modify: `oc-nano/rust/Cargo.toml` — add `oc-nano-history` member
- Test: `oc-nano/rust/oc-nano-history/tests/history_test.rs`

**Interfaces:**
- Consumes: `oc-nano-core::Message`, `oc-nano-core::SessionId`
- Produces: `HistoryStore{conn: Mutex<Connection>, db_path, cache_size_kb}`, `open(path, cache_size_kb) -> HistoryStore`, `close(handle) -> ()`, `store_message(handle, message_json) -> ()`, `load_conversation(handle, session_id) -> String` (streaming JSON), `spill_conversation(handle, session_id, messages_json) -> i64`, `prune_before(handle, cutoff) -> i64`, `prune_old(handle, ttl_seconds) -> i64`, `VACUUM only on prune (not every call)`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_history_open_close_no_leak() {
    let store = HistoryStore::open(":memory:", 8192).unwrap();
    let handle = store.handle();
    HistoryStore::close(handle).unwrap();
    // after close, store_message should error
    assert!(store.store_message(handle, "{}").is_err());
}
```

Run: `cargo test --package oc-nano-history`

Expected: FAIL — `HistoryStore::close` not defined

- [ ] **Step 2: Implement history with close + bounded cache**

`PRAGMA cache_size=-cache_size_kb` (8 MB default, not 64 MB). Streaming `load_conversation_json`. `close()` drops the connection handle. VACUUM only in `prune_old`, not every `prune_before`.

Run: `cargo test --package oc-nano-history`

Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: oc-nano-history crate with close binding and bounded cache"
```

---

### Task 4: oc-nano-schema crate — stable FxHash schema cache with TTL

**Files:**
- Create: `oc-nano/rust/oc-nano-schema/Cargo.toml`
- Create: `oc-nano/rust/oc-nano-schema/src/lib.rs`
- Modify: `oc-nano/rust/Cargo.toml` — add `oc-nano-schema` member
- Test: `oc-nano/rust/oc-nano-schema/tests/schema_test.rs`

**Interfaces:**
- Consumes: `oc-nano-core::ToolSchemaCacheEntry` (renamed)
- Produces: `SchemaCache{DashMap<(String,u64), CachedSchema>}`, `hash_tool_set(tools_json) -> u64` (FxHash, stable across processes), `get(model, hash) -> Option<String>`, `put(model, hash, schema_json)`, `insert_with_ttl(model, hash, schema_json, ttl_secs)`, `evict_expired() -> i64`, `len() -> i64`, `clear()`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_schema_hash_stable_across_restarts() {
    let h1 = hash_tool_set(r#"[{"name":"a"}]"#);
    let h2 = hash_tool_set(r#"[{"name":"a"}]"#);
    assert_eq!(h1, h2);
    assert_ne!(h1, 0);
}
```

Run: `cargo test --package oc-nano-schema`

Expected: FAIL — `hash_tool_set` not defined

- [ ] **Step 2: Implement schema cache with FxHash**

`fxhash::FxHasher` for cross-process stable hashing. `i64` safe cast (u64 > i64::MAX → clamp or error). TTL eviction with `Instant`.

Run: `cargo test --package oc-nano-schema`

Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: oc-nano-schema crate with stable FxHash and TTL eviction"
```

---

### Task 5: oc-nano-state crate — bounded LRU+TTL instance pool

**Files:**
- Create: `oc-nano/rust/oc-nano-state/Cargo.toml`
- Create: `oc-nano/rust/oc-nano-state/src/lib.rs`
- Modify: `oc-nano/rust/Cargo.toml` — add `oc-nano-state` member
- Test: `oc-nano/rust/oc-nano-state/tests/state_test.rs`

**Interfaces:**
- Consumes: none
- Produces: `StatePool{capacity, default_ttl, cache: Mutex<LruCache<String, PooledInstance>>}`, `init(capacity, default_ttl_secs)`, `acquire(instance_id, config_json, ttl_seconds) -> String`, `release(instance_id)`, `peek(instance_id) -> Option<String>`, `prune() -> i64`, `len() -> i64`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_state_pool_lru_eviction() {
    let pool = StatePool::with_capacity(2, 300);
    pool.acquire("a", "cfg1", 300);
    pool.acquire("b", "cfg2", 300);
    pool.acquire("c", "cfg3", 300); // should evict LRU "a"
    assert_eq!(pool.len(), 2);
    assert!(pool.peek("a").is_none());
}
```

Run: `cargo test --package oc-nano-state`

Expected: FAIL — `StatePool` not defined

- [ ] **Step 2: Implement state pool**

`lru::LruCache` with `Mutex`. TTL checked on `acquire` and `prune`. `i64` for all numeric returns.

Run: `cargo test --package oc-nano-state`

Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: oc-nano-state crate with LRU+TTL bounded pool"
```

---

### Task 6: oc-nano-prompt crate — arena-backed messages, streaming attachments

**Files:**
- Create: `oc-nano/rust/oc-nano-prompt/Cargo.toml`
- Create: `oc-nano/rust/oc-nano-prompt/src/lib.rs`
- Modify: `oc-nano/rust/Cargo.toml` — add `oc-nano-prompt` member
- Test: `oc-nano/rust/oc-nano-prompt/tests/prompt_test.rs`

**Interfaces:**
- Consumes: `oc-nano-core::Message`, `oc-nano-core::SessionId`, `oc-nano-core::MsgRole`
- Produces: `PromptArena{messages, conversations, attachments}`, `init()`, `add_message(content, role) -> u64`, `materialize(conversation_handle: i64) -> String`, `attachment_stream(attachment_id) -> Vec<u8>` (raw bytes, no base64), `enforce_ram_cap(conversation_handle) -> i64` (returns count over cap)

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_prompt_arena_arc_str_no_clone() {
    let arena = PromptArena::new();
    let id = arena.add_message("hello".to_string(), MsgRole::User);
    let c1 = arena.get_message(id).unwrap();
    let c2 = arena.get_message(id).unwrap();
    assert!(std::ptr::eq(&*c1, &*c2)); // same Arc<str>
}
```

Run: `cargo test --package oc-nano-prompt`

Expected: FAIL — `PromptArena` not defined

- [ ] **Step 2: Implement prompt arena**

`DashMap<u64, StoredMessage>` with `Arc<str>` content. Raw `Vec<u8>` attachments. `materialize` formats `[role] content` + `[attachment] id` lines. `enforce_ram_cap` checks message count against `DEFAULT_RAM_MESSAGE_CAP`.

Run: `cargo test --package oc-nano-prompt`

Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: oc-nano-prompt crate with Arc<str> arena and raw attachments"
```

---

### Task 7: oc-nano-compaction crate — streaming single-pass compaction (no full-RAM load)

**Files:**
- Create: `oc-nano/rust/oc-nano-compaction/Cargo.toml`
- Create: `oc-nano/rust/oc-nano-compaction/src/lib.rs`
- Modify: `oc-nano/rust/Cargo.toml` — add `oc-nano-compaction` member
- Test: `oc-nano/rust/oc-nano-compaction/tests/compaction_test.rs`

**Interfaces:**
- Consumes: `messages_json: String` (streaming parse), `target_bytes: i64`
- Produces: `compact_spill(messages_json, target_bytes) -> String` (single pass: summarize old + keep suffix), `compact_prune(messages_json, target_bytes) -> String` (drop old + keep suffix), `COMPACTION_OVERHEAD`, `SUMMARY_EST`, `WRAPPER` constants

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_compaction_single_pass_bounded() {
    let msgs = r#"[{"role":"user","content":"hello"}]"#;
    let result = compact_spill(msgs, 1000);
    assert!(result.len() <= 1000 + COMPACTION_OVERHEAD as usize);
}
```

Run: `cargo test --package oc-nano-compaction`

Expected: FAIL — `compact_spill` not defined

- [ ] **Step 2: Implement streaming compaction**

Parse messages_json streaming (serde_json::Deserializer::from_str). Single pass: summarize older messages into one system message, keep recent suffix verbatim. No `while(true)` loop. Fixed target size up front.

Run: `cargo test --package oc-nano-compaction`

Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: oc-nano-compaction crate with streaming single-pass compaction"
```

---

### Task 8: oc-nano-instructions crate — path-interned file contents

**Files:**
- Create: `oc-nano/rust/oc-nano-instructions/Cargo.toml`
- Create: `oc-nano/rust/oc-nano-instructions/src/lib.rs`
- Modify: `oc-nano/rust/Cargo.toml` — add `oc-nano-instructions` member
- Test: `oc-nano/rust/oc-nano-instructions/tests/instructions_test.rs`

**Interfaces:**
- Consumes: `file_path: String`
- Produces: `InstructionsStore{inner: DashMap<String, Arc<str>>}`, `load(path) -> Arc<str>` (read once, intern), `evict(path) -> bool`, `len() -> i64`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_instructions_intern_by_path() {
    let store = InstructionsStore::new();
    let c1 = store.load("/tmp/test.md").unwrap();
    let c2 = store.load("/tmp/test.md").unwrap();
    assert!(std::ptr::eq(&*c1, &*c2));
}
```

Run: `cargo test --package oc-nano-instructions`

Expected: FAIL — `InstructionsStore` not defined

- [ ] **Step 2: Implement instructions store**

`DashMap<String, Arc<str>>` with `entry()` API for single-read under concurrency. Read file once, intern as `Arc<str>`.

Run: `cargo test --package oc-nano-instructions`

Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: oc-nano-instructions crate with path-interned file contents"
```

---

### Task 9: oc-nano-napi crate — all bindings with panic boundaries, aligned signatures

**Files:**
- Create: `oc-nano/rust/oc-nano-napi/Cargo.toml`
- Create: `oc-nano/rust/oc-nano-napi/src/lib.rs`
- Modify: `oc-nano/rust/Cargo.toml` — add `oc-nano-napi` member
- Test: `oc-nano/rust/oc-nano-napi/tests/napi_test.rs`

**Interfaces:**
- Consumes: all 6 other crates
- Produces: All `#[napi]` bindings wrapped in `napi_panic_boundary!`:
  - `binary_name() -> String`, `config_dir_name() -> String`, `data_dir_name() -> String`, `version_check() -> String`
  - `history_init(path, cache_size_kb) -> i64`, `history_close(handle) -> ()`, `history_store_message(handle, message_json) -> ()`, `history_load_conversation(handle, session_id) -> String`, `history_spill_conversation(handle, session_id, messages_json) -> i64`, `history_prune_old(handle, ttl_seconds) -> i64`
  - `schema_cache_hash(tools_json) -> i64`, `schema_cache_get(model, hash) -> Option<String>`, `schema_cache_put(model, hash, schema_json) -> ()`, `schema_cache_evict_expired() -> i64`
  - `instructions_load(path) -> String`, `instructions_evict(path) -> bool`
  - `state_pool_init(capacity, default_ttl_secs) -> ()`, `state_acquire(instance_id, config_json, ttl_seconds) -> String`, `state_release(instance_id) -> ()`, `state_prune() -> i64`, `state_len() -> i64`
  - `prompt_init() -> ()`, `prompt_materialize(conversation_handle: i64) -> String`, `prompt_attachment_stream(attachment_id) -> Vec<u8>`, `prompt_enforce_ram_cap(conversation_handle: i64) -> i64`
  - `compact_spill(messages_json, target_bytes) -> String`, `compact_prune(messages_json, target_bytes) -> String`

All numerics `i64`. All bindings wrapped in `napi_panic_boundary!`. `to_napi_error` returns friendly messages (not `{:?}` debug). `schema_cache_hash` returns `i64` clamped from `u64`.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_napi_bindings_exist() {
    // all bindings are #[napi] exported functions
    // verify they compile and have correct signatures
}
```

Run: `cargo check --package oc-nano-napi`

Expected: PASS (all bindings compile)

- [ ] **Step 2: Implement all napi bindings**

Each binding wrapped in `napi_panic_boundary!`. Signature alignment with TS wrapper. `history_close` added. `schema_cache_hash` uses FxHash. `i64` safe cast.

Run: `cargo build --release`

Expected: `oc_nano_native.dll` built cleanly, zero warnings

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: oc-nano-napi crate with panic-safe bindings and aligned signatures"
```

---

### Task 10: TS wrapper + integration docs (completely renamed)

**Files:**
- Create: `oc-nano/src/napi-bindings.ts` — `OcNanoNative` interface, `loadNanoNapi()`, `getNanoNapi()`, DLL from `OC_NANO_DLL` env var
- Create: `oc-nano/scripts/integrate.ps1` — copy DLL, verify wrapper
- Create: `oc-nano/docs/integration/TS_SIDE_CHANGES.md` — exact per-hot-path TS edits with new names
- Test: `bun --typecheck src/napi-bindings.ts`

**Interfaces:**
- Consumes: `oc_nano_native.dll`
- Produces: TS wrapper matching all Rust binding signatures exactly

- [ ] **Step 1: Create TS wrapper**

```typescript
interface OcNanoNative {
  binary_name(): Promise<string>;
  config_dir_name(): Promise<string>;
  data_dir_name(): Promise<string>;
  version_check(): Promise<string>;
  // ... all bindings with exact Rust signature alignment
}
```

Run: `bun --typecheck src/napi-bindings.ts`

Expected: no type errors

- [ ] **Step 2: Create integration docs**

Document all 6 hot paths with new names (prompt.ts → nano-prompt.ts, etc.), exact line numbers, before/after snippets using `oc-nano` naming.

Run: verify doc exists

Expected: TS_SIDE_CHANGES.md complete

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: TS wrapper + integration docs with oc-nano naming"
```

---

### Task 11: Full build verification + smoke test

**Files:**
- Modify: none (verification only)
- Test: `oc-nano/rust/target/release/oc_nano_native.dll`

**Interfaces:**
- Consumes: all 7 crates built
- Produces: verified DLL, all tests passing, no warnings

- [ ] **Step 1: Clean and full build**

Run: `cargo clean && cargo build --release` from `oc-nano/rust/`

Expected: all 7 crates compile, `oc_nano_native.dll` emitted

- [ ] **Step 2: Run all tests**

Run: `cargo test --release` from `oc-nano/rust/`

Expected: all tests pass across all crates

- [ ] **Step 3: Verify DLL loadability**

Run: check file exists, size > 0, can be loaded

Expected: `oc_nano_native.dll` (~3 MB) present and valid

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "feat: full oc-nano build verified, all tests passing"
```
