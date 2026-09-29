//! plan-20260927 GC-CM-12: live-cloud resource helpers.
//!
//! Compiles `tests/helpers/cloud_live_resources` (writer-slot reads, identity
//! probes, pre-allocation scoping) and runs its unit tests in the default L1
//! suite (no real D1/R2 needed). The live-cloud tests consume the same module.

mod helpers;

#[cfg(test)]
mod tests {
    use crate::helpers::cloud_live_resources::resource_identity_probe;

    /// Smoke test for the live-cloud resource helper.
    ///
    /// Both branches are local-only and never contact D1/R2, which is why this
    /// stays in the default L1 suite:
    ///
    /// - with `LIBRA_D1_ACCOUNT_ID` configured, the probe must resolve a
    ///   non-empty identity;
    /// - without it, the probe must fail closed rather than hand a live test an
    ///   empty identity it could mistake for a verified one.
    ///
    /// Asserting only one of the two branches makes `cargo test --all` depend on
    /// the caller's environment; assert both instead.
    #[test]
    fn cloud_live_resources_helper_compiles() {
        let configured =
            std::env::var("LIBRA_D1_ACCOUNT_ID").is_ok_and(|value| !value.trim().is_empty());

        if configured {
            let identity = resource_identity_probe(None);
            assert!(
                !identity.d1_account_id.is_empty(),
                "configured LIBRA_D1_ACCOUNT_ID must resolve to a non-empty identity"
            );
            return;
        }

        // Silence the expected panic's output while asserting the fail-closed arm.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| resource_identity_probe(None));
        std::panic::set_hook(hook);

        assert!(
            outcome.is_err(),
            "resource_identity_probe must fail closed when LIBRA_D1_ACCOUNT_ID is unset"
        );
    }
}
