//! Panic-safe wrapper for napi-rs binding function bodies.
//!
//! The workspace release profile is compiled with `panic = "abort"`, so a panic
//! inside an napi binding would tear down the entire host process. Individual
//! bindings therefore wrap their bodies in `std::panic::catch_unwind` and
//! translate any panic into a graceful `napi::Error` instead of aborting.

use napi::Result;

/// Run closure `f` under `catch_unwind`. On success the returned value is
/// promoted to `Ok`. On panic, a graceful `napi::Error::from_reason("internal
/// panic")` is returned instead of aborting the host.
///
/// The closure returns the raw success value `R`; `wrap_napi` promotes it to
/// `napi::Result<R>`.
pub fn wrap_napi<F, R>(f: F) -> napi::Result<R>
where
    F: FnOnce() -> R + std::panic::UnwindSafe,
{
    match std::panic::catch_unwind(f) {
        Ok(value) => Ok(value),
        Err(_panic) => Err(napi::Error::from_reason("internal panic")),
    }
}

/// Wrap the body of an napi binding function so panics are caught and turned
/// into graceful `napi::Error`s rather than aborting the host.
///
/// The block should evaluate to `napi::Result<R>`. On success the result is
/// returned as-is; on panic the block returns early with
/// `Err(napi::Error::from_reason("internal panic"))`.
///
/// Usage:
///
/// ```ignore
/// #[napi]
/// fn my_binding(input: String) -> napi::Result<String> {
///     panic_guard!({
///         let processed = do_work(input)?;
///         Ok(processed)
///     })
/// }
/// ```
#[macro_export]
macro_rules! panic_guard {
    ($body:expr) => {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| $body)) {
            Ok(result) => result,
            Err(_panic) => return Err(napi::Error::from_reason("internal panic")),
        }
    };
}
