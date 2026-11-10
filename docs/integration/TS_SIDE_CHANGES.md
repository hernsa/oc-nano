# TS side changes — oc-nano napi integration

This document records the exact per-hot-path TypeScript edits required to swap
OpenCode's bloated memory hot paths for the oc-nano native bindings exposed in
`src/napi-bindings.ts` (28 #[napi] bindings over `oc_nano_native.dll`).

**NOTE:** The OpenCode repository source is **not** available locally (C drive
critically low; no clone / no large downloads permitted). These edits are
therefore **documentation only** — they must be applied against the OpenCode
repo once it is available. All file paths and line numbers below assume the
OpenCode checkout layout (`packages/core/src/...`).

The TypeScript wrapper itself lives at `src/napi-bindings.ts` and exports
`OcNanoNapi`, `loadNapi(dllPath?)`, and `getNapi()`.

---

## Hot path 1 — Prompt assembly: `structuredClone` + attachment base64 re-encode

**File:** `packages/core/src/session/prompt.ts`
**Line ranges:** `34-162`, `172-222`

**What to replace:** The current implementation deep-clones the conversation
message list with `structuredClone(...)` and re-encodes attachment payloads as
base64 strings before materialising the prompt. Lines 34-162 build/clone the
message array; lines 172-222 handle attachment resolution and re-encoding.

**What napi binding replaces it:**
- `prompt_init()` — initialise the prompt arena once at module load.
- `prompt_add_message(conversation_id, role, content) -> i64` — append each
  message to the arena instead of cloning into a JS array.
- `prompt_materialize(conversation_id) -> string` — materialise the full
  prompt from the arena in one native call (replaces the JS-side render).
- `prompt_add_attachment(attachment_id, bytes: Uint8Array) -> void` — register
  raw attachment bytes once (no base64 round-trip).
- `prompt_get_attachment(attachment_id) -> Promise<Uint8Array | null>` — fetch
  raw attachment bytes on demand (replaces base64 re-encode/decode).

**Notes:** `conversation_id` crosses FFI as an `i64` internal handle (see the
napi-rs crate comment at the top of `rust/oc-nano-napi/src/lib.rs`). The TS
side must map OpenCode conversation ids to arena handles.

---

## Hot path 2 — Compaction: full-history re-materialisation per shrink iteration

**File:** `packages/core/src/session/compaction.ts`
**Line range:** `298-785`

**What to replace:** The compaction loop re-materialises the entire conversation
history on every shrink iteration (JSON round-trip, then in-memory filtering
and re-serialisation), which dominates memory and CPU for long conversations.

**What napi binding replaces it:**
- `compact_spill(conversation_id, target_bytes, messages_json) -> string` —
  spill/shrink messages to `target_bytes` in one native call; replaces the
  per-iteration re-materialisation.
- `compact_prune_old(conversation_id, target_bytes, messages_json) -> string` —
  prune oldest messages to `target_bytes` in one native call.

**Notes:** Both bindings take `messages_json` (serialised `NanoMessage[]`) and
return the compacted JSON string. The native `compact_*` signatures do not take
`conversation_id`; the TS wrapper threads it through for caller-side scoping
(see `src/napi-bindings.ts` notes). Feed the conversation's current message
JSON once per iteration rather than rebuilding the history from scratch.

---

## Hot path 3 — Tool inputSchema deep-copy

**File:** `packages/core/src/tool.ts`
**Line ranges:** `254-327`, `398-420`

**What to replace:** `JSON.parse(JSON.stringify(inputSchema))` deep-copies of the
tool input schema on every tool invocation, keyed per (model, tool set).

**What napi binding replaces it:**
- `schema_cache_hash(tools_json: string) -> i64` — hash the tool set JSON to a
  stable `tool_set_hash`.
- `schema_cache_get(model, tool_set_hash) -> Promise<string | null>` — fetch a
  cached schema JSON; returns `null` on miss.
- `schema_cache_put(model, tool_set_hash, schema_json, ttl_secs) -> void` —
  store the schema JSON with a TTL.
- `schema_cache_evict_expired() -> i64` — periodic eviction of expired entries.

**Pattern:** On tool resolution, compute `hash = schema_cache_hash(tools_json)`;
`get = schema_cache_get(model, hash)`; if `null`, build the schema and
`schema_cache_put(model, hash, schema_json, ttl_secs)`. Lines 254-327 and
398-420 are the deep-copy sites to replace with the get/put flow.

---

## Hot path 4 — Instructions: unbounded concurrent file reads

**File:** `packages/core/src/session/instructions.ts`
**Line range:** `~47-92`

**What to replace:** Unbounded concurrent reads of system-instruction files from
disk on every request (no cache, no eviction, no dedup of in-flight reads).

**What napi binding replaces it:**
- `instructions_load(path: string) -> Promise<string>` — load and cache the
  instruction file content natively (dedups concurrent reads).
- `instructions_evict(path: string) -> boolean` — evict a path from the native
  cache when it changes / is no longer needed.

**Notes:** Lines ~47-92 are the file-read sites; replace each direct `fs.readFile`
of an instruction file with `instructions_load(path)`, and call
`instructions_evict(path)` on rotation/invalidation.

---

## Hot path 5 — MCP session lifetime maps

**File A:** `packages/core/src/mcp/pool.ts` — line range `44-99`
**File B:** `packages/core/src/mcp/index.ts` — line range `222-686`

**What to replace:** The ad-hoc lifetime maps (`slots`, `entries`, `unpoolable`,
`legacy` maps) tracking MCP session/mcp-client lifetime, pooling, and eviction.

**What napi binding replaces it:**
- `state_pool_init(capacity, default_ttl_secs) -> void` — initialise the
  default state pool once.
- `state_acquire(instance_id, config_json, ttl_secs) -> Promise<string>` —
  acquire/replace a session entry (replaces manual `slots`/`entries` upsert).
- `state_release(instance_id) -> void` — release an entry (replaces
  `unpoolable`/`legacy` removal).
- `state_prune() -> i64` — prune expired entries.
- `state_len() -> i64` — current live entry count.

**Notes:** The native `state_pool_*` fns take a `pool_id`; the TS wrapper
holds the default pool id internally (see `src/napi-bindings.ts`). Lines
222-686 in `index.ts` are the call sites that manipulate the lifetime maps and
should be routed through `state_acquire`/`state_release`/`state_prune`/
`state_len`.

---

## Hot path 6 — Database connection cache size

**File:** `packages/core/src/database/database.ts`
**Line range:** `29-34`

**What to replace:** `PRAGMA cache_size=-64000` (64 MB per connection) set on
every database open — this is the largest per-connection memory consumer.

**What napi binding replaces it:**
- `history_init(db_path, cache_size_kb=8192) -> i64` — open the history store
  with an 8 MB cache (`cache_size_kb=8192`) instead of 64 MB; replaces the
  `PRAGMA cache_size=-64000` + `db_open` flow.
- `history_close(handle) -> void` — close the store on teardown (replaces the
  connection close path).

**Notes:** `cache_size_kb=8192` ≈ 8 MB, an 8× reduction versus the previous
64 MB per connection. `history_init` returns a handle used by the history
bindings (4-10 above); `history_close` releases it.

---

## Application checklist

- [ ] `packages/core/src/session/prompt.ts`  (34-162, 172-222)
- [ ] `packages/core/src/session/compaction.ts` (298-785)
- [ ] `packages/core/src/tool.ts` (254-327, 398-420)
- [ ] `packages/core/src/session/instructions.ts` (~47-92)
- [ ] `packages/core/src/mcp/pool.ts` (44-99) + `packages/core/src/mcp/index.ts` (222-686)
- [ ] `packages/core/src/database/database.ts` (29-34)
- [ ] Add `src/napi-bindings.ts` to the OpenCode TS build / import it at the
  host entry point and call `loadNapi()` before any hot path runs.
- [ ] Confirm `bun --typecheck` passes on `src/napi-bindings.ts`.
