# oc-nano Architecture

oc-nano is a lean replacement for OpenCode's memory-bloating hot paths, built
as an `napi-rs` Rust workspace so the Node host can call into small,
tightly-scoped native modules.

## Workspace layout

- `oc-nano-core` — primitives, result/error types, shared utilities.
- `oc-nano-history` — session history persistence and replay.
- `oc-nano-schema` — JSON schemas and validation for prompts/state.
- `oc-nano-state` — in-memory and on-disk state machine.
- `oc-nano-prompt` — prompt assembly and template rendering.
- `oc-nano-compaction` — history compaction and summarisation.
- `oc-nano-instructions` — system instruction handling.
- `oc-nano-napi` — napi-rs bindings exposed to the Node host.

## Panic safety

The release profile uses `panic = "abort"` for minimal binary size and
maximum speed. Because an abort would tear down the host process, individual
napi bindings wrap their bodies with `panic_guard!` (see `rust/src/panic_guard.rs`),
which catches panics via `std::panic::catch_unwind` and returns a graceful
`napi::Error::from_reason("internal panic")` instead.

## Build

From `oc-nano/rust/` (MinGW on `PATH`):

```sh
rustup run stable-x86_64-pc-windows-gnu cargo build --release
```

The native library produced by `oc-nano-napi` is `oc_nano_native.dll`.
