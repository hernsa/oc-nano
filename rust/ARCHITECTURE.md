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

The release profile uses `panic = "unwind"` so a panic can be caught instead of
tearing down the host process. Individual napi bindings wrap their bodies with
`panic_guard!` (see `rust/src/panic_guard.rs`), which catches panics via
`std::panic::catch_unwind` and returns a graceful
`napi::Error::from_reason("internal panic")` instead. The profile must stay
`unwind`: `abort` would make that guard useless.

## Build

The shipped addon is built by CI on `windows-latest` with the MSVC toolchain
(`.github/workflows/build-addon.yml`), because the addon has to link against the
Node headers for the exact runtime:

```sh
rustup run stable-x86_64-pc-windows-msvc cargo build --release --target x86_64-pc-windows-msvc --package oc-nano-napi
```

The native library produced by `oc-nano-napi` is `oc_nano_native.dll`. A local
MinGW build compiles the same crate into a different binary and is not shipped:

```sh
rustup run stable-x86_64-pc-windows-gnu cargo build --release
```
