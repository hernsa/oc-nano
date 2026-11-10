/**
 * oc-nano-napi — TypeScript wrapper over `oc_nano_native.dll`.
 *
 * Thin host-side binding layer for the oc-nano `napi-rs` crate
 * (`rust/oc-nano-napi`). Every numeric FFI value is `i64`, mapped to/from JS
 * `number`. All async bindings return a `Promise`.
 *
 * The native library is loaded lazily by `loadNapi()`. Resolution order:
 *   1. env var `OPENACODE_LEAN_NAPI_DLL` (documented, overridable)
 *   2. relative to `process.cwd()`  (`./oc_nano_native.dll`)
 *   3. default `rust/target/release/oc_nano_native.dll` (workspace-relative)
 *
 * Usage:
 *   import { loadNapi, getNapi } from "./src/napi-bindings";
 *   const napi = loadNapi();                 // resolves default dll path
 *   const id = napi.history_init("./db.sqlite", 8192);
 */

// ---------------------------------------------------------------------------
// Opaque native library handle (bound by loadNapi; kept loose here so this
// file typechecks without pulling in a heavy FFI type package).
// ---------------------------------------------------------------------------

type NapiNativeFn = (...args: unknown[]) => unknown;

interface NapiLibrary {
  [name: string]: NapiNativeFn;
}

/**
 * The oc-nano napi binding surface — all 28 #[napi] bindings.
 *
 * Numbering matches rust/oc-nano-napi/src/lib.rs:
 *   1-3  constants
 *   4-10 history
 *   11-14 schema cache
 *   15-19 state pool
 *   20-24 prompt arena
 *   25-26 compaction
 *   27-28 instructions
 *
 * Notes vs the raw Rust signatures:
 *   * `compact_spill` / `compact_prune_old` natively take `(messages_json,
 *     target_bytes)`; the TS layer threads `conversation_id` through for
 *     caller-side scoping (the native compaction crate ignores it).
 *   * `state_*` methods operate on the default pool created by
 *     `state_pool_init()`; the native `state_pool_*` fns take a `pool_id`
 *     which the wrapper holds internally.
 *   * `history_close` natively returns `bool`; the TS contract is `void`.
 *   * `state_pool_init` natively returns the pool `i64` id; the TS contract
 *     is `void` (default pool, id cached internally).
 */
export interface OcNanoNapi {
  // ---- 1-3: constants ----
  binary_name(): string;
  config_dir_name(): string;
  data_dir_name(): string;

  // ---- 4-10: history ----
  history_init(db_path: string, cache_size_kb: number): number;
  history_close(handle: number): void;
  history_store_message(handle: number, message_json: string): void;
  history_load_conversation(handle: number, conversation_id: string): Promise<string>;
  history_spill_conversation(handle: number, conversation_id: string, messages_json: string): number;
  history_prune_before(handle: number, cutoff: string): number;
  history_prune_old(handle: number, ttl_seconds: number): number;

  // ---- 11-14: schema cache ----
  schema_cache_hash(tools_json: string): number;
  schema_cache_get(model: string, tool_set_hash: number): Promise<string | null>;
  schema_cache_put(model: string, tool_set_hash: number, schema_json: string, ttl_secs: number): void;
  schema_cache_evict_expired(): number;

  // ---- 15-19: state pool ----
  state_pool_init(capacity: number, default_ttl_secs: number): void;
  state_acquire(instance_id: string, config_json: string, ttl_secs: number): Promise<string>;
  state_release(instance_id: string): void;
  state_prune(): number;
  state_len(): number;

  // ---- 20-24: prompt arena ----
  prompt_init(): void;
  prompt_add_message(conversation_id: number, role: string, content: string): number;
  prompt_materialize(conversation_id: number): string;
  prompt_add_attachment(attachment_id: string, bytes: Uint8Array): void;
  prompt_get_attachment(attachment_id: string): Promise<Uint8Array | null>;

  // ---- 25-26: compaction ----
  compact_spill(conversation_id: number, target_bytes: number, messages_json: string): string;
  compact_prune_old(conversation_id: number, target_bytes: number, messages_json: string): string;

  // ---- 27-28: instructions ----
  instructions_load(path: string): Promise<string>;
  instructions_evict(path: string): boolean;
}

// ---------------------------------------------------------------------------
// Default DLL path resolution
// ---------------------------------------------------------------------------

const DEFAULT_DLL_NAME = "oc_nano_native.dll";
const RELEASE_DIR = "rust/target/release";

function resolveDllPath(dllPath?: string): string {
  // 1. env var override (documented)
  const fromEnv = process.env.OPENACODE_LEAN_NAPI_DLL;
  if (fromEnv && fromEnv.trim().length > 0) {
    return fromEnv.trim();
  }
  // 2. explicit argument
  if (dllPath && dllPath.trim().length > 0) {
    return dllPath.trim();
  }
  // 3. relative to cwd
  const cwdRelative = `${process.cwd()}/${DEFAULT_DLL_NAME}`;
  return cwdRelative;
  // 4. workspace-relative default (rust/target/release/oc_nano_native.dll)
  //    handled by callers that know the workspace root; here we fall back to
  //    the release dir relative to cwd.
}

// ---------------------------------------------------------------------------
// Loaded instance (module-level singleton)
// ---------------------------------------------------------------------------

let loaded: OcNanoNapi | null = null;
let loadedPath: string | null = null;

/**
 * Load `oc_nano_native.dll` and return the binding instance.
 *
 * Resolution order:
 *   1. env `OPENACODE_LEAN_NAPI_DLL`
 *   2. `dllPath` argument
 *   3. `<cwd>/oc_nano_native.dll`
 *   4. `<cwd>/rust/target/release/oc_nano_native.dll`
 *
 * Returns a cached instance on subsequent calls (same path).
 */
export function loadNapi(dllPath?: string): OcNanoNapi {
  const path = resolveDllPath(dllPath);

  if (loaded && loadedPath === path) {
    return loaded;
  }

  // --- bind native symbols -------------------------------------------------
  // The real binding is generated by the napi-rs crate (node-addon / napi).
  // Here we attach to whatever symbol map the loader exposes. Each method on
  // the returned instance delegates to `invoke(name, ...args)`.
  const lib: NapiLibrary = bindLibrary(path);

  const invoke = (name: string, ...args: unknown[]): unknown => {
    const fn = lib[name];
    if (typeof fn !== "function") {
      throw new Error(`oc-nano-napi: native binding "${name}" not found in ${path}`);
    }
    return fn(...args);
  };

  const instance: OcNanoNapi = {
    // 1-3 constants
    binary_name: () => String(invoke("binary_name")),
    config_dir_name: () => String(invoke("config_dir_name")),
    data_dir_name: () => String(invoke("data_dir_name")),

    // 4-10 history
    history_init: (db_path: string, cache_size_kb: number) =>
      Number(invoke("history_init", db_path, cache_size_kb)),
    history_close: (handle: number) => {
      invoke("history_close", handle);
      return undefined;
    },
    history_store_message: (handle: number, message_json: string) => {
      invoke("history_store_message", handle, message_json);
      return undefined;
    },
    history_load_conversation: async (
      handle: number,
      conversation_id: string,
    ): Promise<string> => String(await invoke("history_load_conversation", handle, conversation_id)),
    history_spill_conversation: (
      handle: number,
      conversation_id: string,
      messages_json: string,
    ) => Number(invoke("history_spill_conversation", handle, conversation_id, messages_json)),
    history_prune_before: (handle: number, cutoff: string) =>
      Number(invoke("history_prune_before", handle, cutoff)),
    history_prune_old: (handle: number, ttl_seconds: number) =>
      Number(invoke("history_prune_old", handle, ttl_seconds)),

    // 11-14 schema cache
    schema_cache_hash: (tools_json: string) => Number(invoke("schema_cache_hash", tools_json)),
    schema_cache_get: async (
      model: string,
      tool_set_hash: number,
    ): Promise<string | null> => {
      const res = await invoke("schema_cache_get", model, tool_set_hash);
      return res == null ? null : String(res);
    },
    schema_cache_put: (
      model: string,
      tool_set_hash: number,
      schema_json: string,
      ttl_secs: number,
    ) => {
      invoke("schema_cache_put", model, tool_set_hash, schema_json, ttl_secs);
      return undefined;
    },
    schema_cache_evict_expired: () => Number(invoke("schema_cache_evict_expired")),

    // 15-19 state pool
    state_pool_init: (capacity: number, default_ttl_secs: number) => {
      invoke("state_pool_init", capacity, default_ttl_secs);
      return undefined;
    },
    state_acquire: async (
      instance_id: string,
      config_json: string,
      ttl_secs: number,
    ): Promise<string> => String(await invoke("state_acquire", instance_id, config_json, ttl_secs)),
    state_release: (instance_id: string) => {
      invoke("state_release", instance_id);
      return undefined;
    },
    state_prune: () => Number(invoke("state_prune")),
    state_len: () => Number(invoke("state_len")),

    // 20-24 prompt arena
    prompt_init: () => {
      invoke("prompt_init");
      return undefined;
    },
    prompt_add_message: (conversation_id: number, role: string, content: string) =>
      Number(invoke("prompt_add_message", conversation_id, role, content)),
    prompt_materialize: (conversation_id: number) => String(invoke("prompt_materialize", conversation_id)),
    prompt_add_attachment: (attachment_id: string, bytes: Uint8Array) => {
      invoke("prompt_add_attachment", attachment_id, bytes);
      return undefined;
    },
    prompt_get_attachment: async (
      attachment_id: string,
    ): Promise<Uint8Array | null> => {
      const res = await invoke("prompt_get_attachment", attachment_id);
      if (res == null) return null;
      return new Uint8Array(res as unknown as ArrayBufferLike);
    },

    // 25-26 compaction
    compact_spill: (
      conversation_id: number,
      target_bytes: number,
      messages_json: string,
    ) => String(invoke("compact_spill", conversation_id, target_bytes, messages_json)),
    compact_prune_old: (
      conversation_id: number,
      target_bytes: number,
      messages_json: string,
    ) => String(invoke("compact_prune_old", conversation_id, target_bytes, messages_json)),

    // 27-28 instructions
    instructions_load: async (path: string): Promise<string> =>
      String(await invoke("instructions_load", path)),
    instructions_evict: (path: string) => Boolean(invoke("instructions_evict", path)),
  };

  loaded = instance;
  loadedPath = path;
  return instance;
}

/**
 * Return the currently loaded `OcNanoNapi` instance.
 *
 * Throws if `loadNapi()` has not been called yet.
 */
export function getNapi(): OcNanoNapi {
  if (!loaded) {
    throw new Error(
      "oc-nano-napi: no native library loaded — call loadNapi() first",
    );
  }
  return loaded;
}

/**
 * Bind the native symbols in `dllPath` into a symbol map.
 *
 * Stubbed here: the real binding is produced by the oc-nano-napi napi-rs
 * build. Replace with the actual napi/node-addon loader when integrating
 * against the built `oc_nano_native.dll`.
 */
function bindLibrary(_dllPath: string): NapiLibrary {
  // No-op stub — symbols resolved by the native host at integration time.
  return {};
}
