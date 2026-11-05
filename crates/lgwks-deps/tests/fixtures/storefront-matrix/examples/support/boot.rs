//! The shared start of the journeys that make TLS clients: one ring provider,
//! installed twice to prove the second call is a no-op, and one owned runtime.

/// Install ring, build the runtime, drive `journey`; false on any refusal.
pub(crate) fn run_with_tls<F: std::future::Future<Output = bool>>(journey: F) -> bool {
    let installed = lgwks_deps::tls::install_ring().and_then(|()| lgwks_deps::tls::install_ring());
    if installed.is_err() {
        return false;
    }
    let Ok(runtime) = lgwks_bot::rt::runtime::Runtime::new() else {
        return false;
    };
    runtime.block_on(journey)
}
