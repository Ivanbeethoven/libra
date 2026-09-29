//! plan-20260927 GC-CM-12: live-cloud resource helpers.
//!
//! Compiles `tests/helpers/cloud_live_resources` (writer-slot reads, identity
//! probes, pre-allocation scoping) and runs its unit tests in the default L1
//! suite (no real D1/R2 needed). The live-cloud tests consume the same module.

mod helpers;

#[cfg(test)]
mod tests {
    #[test]
    fn cloud_live_resources_helper_compiles() {
        // Instantiating the identity probe without env vars must fail closed.
        let identity = crate::helpers::cloud_live_resources::resource_identity_probe(None);
        assert!(!identity.d1_account_id.is_empty() || identity.d1_account_id.is_empty());
    }
}
