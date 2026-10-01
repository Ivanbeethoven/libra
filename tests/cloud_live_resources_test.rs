//! plan-20260927 GC-CM-12: live-cloud resource helpers.
//!
//! Compiles `tests/helpers/cloud_live_resources` (writer-slot reads, identity
//! probes, pre-allocation scoping) and runs its unit tests in the default L1
//! suite (no real D1/R2 needed). The live-cloud tests consume the same module.

mod helpers;

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use crate::helpers::{
        cloud_live_manifest::validate_writer_slots,
        cloud_live_resources::{WriterSlot, resource_identity_probe, slot_is_live},
    };

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

    /// plan-20260927 G-3e: the live-cloud provisioner must satisfy its own
    /// validators.
    ///
    /// `tests/cloud_live_prepare.sh` is the only producer of `slots.json`, while
    /// `load_writer_slots` / `validate_writer_slots` are its only consumers.
    /// Running it for real (rather than `bash -n`) is what exposed three defects:
    /// unsorted `repo_id`s rejected by the strict-ascending validator, deadlines
    /// minted in the past, and `slots.env` drifting out of sync with the
    /// re-sorted slots. Pinned here so the G-3a/G-3e wiring cannot regress it
    /// silently, with no real D1/R2 contact.
    #[test]
    fn cloud_live_prepare_script_emits_canonical_slots_with_future_deadline() {
        const TTL_SECONDS: u64 = 600;
        const SLACK_SECONDS: u64 = 120;

        let sandbox = tempfile::tempdir().expect("tempdir");
        let slots_dir = sandbox.path().join("slots");
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/cloud_live_prepare.sh");
        let run = || {
            std::process::Command::new("bash")
                .arg(script)
                .arg("--slots-dir")
                .arg(&slots_dir)
                .arg("--count")
                .arg("4")
                .env("LIBRA_CLOUD_LIVE_SLOT_TTL_SECONDS", TTL_SECONDS.to_string())
                .output()
                .expect("run tests/cloud_live_prepare.sh")
        };

        let first = run();
        assert!(
            first.status.success(),
            "provisioner must succeed: stdout={} stderr={}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );

        let slots_json = std::fs::read_to_string(slots_dir.join("slots.json"))
            .expect("provisioner must write slots.json");
        let payload: serde_json::Value =
            serde_json::from_str(&slots_json).expect("slots.json must be JSON");
        let slots = payload
            .as_array()
            .expect("slots.json is the machine-readable slot array")
            .clone();
        assert_eq!(slots.len(), 4, "provisioner must honour --count");

        // The producer's own validator must accept the product (GC-CM-15).
        validate_writer_slots(&serde_json::json!({ "writer_slots": slots }))
            .unwrap_or_else(|err| panic!("provisioner output rejected by validator: {err}"));

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock must be after the epoch")
            .as_secs();
        let mut repo_ids = Vec::new();
        for slot in &slots {
            let slot: WriterSlot = serde_json::from_value(slot.clone()).expect("writer slot");
            assert_eq!(
                slot.r2_prefix,
                format!("{}/", slot.repo_id),
                "r2_prefix must be exactly `<repo_id>/`"
            );
            assert!(
                slot.repo_id.starts_with("test-repo-"),
                "repo_id must use the `test-repo-` shape, got `{}`",
                slot.repo_id
            );
            assert!(
                slot_is_live(&slot),
                "a freshly minted slot must be live, got expires_at={}",
                slot.expires_at
            );
            let naive = chrono::NaiveDateTime::parse_from_str(
                slot.expires_at
                    .strip_suffix('Z')
                    .expect("deadline must be UTC-suffixed"),
                "%Y-%m-%dT%H:%M:%S",
            )
            .expect("deadline must parse");
            let deadline = naive.and_utc().timestamp().max(0) as u64;
            assert!(
                deadline > now,
                "deadline must be in the future, otherwise slot_is_live rejects every run: \
                 {deadline} <= {now}"
            );
            assert!(
                deadline <= now + TTL_SECONDS + SLACK_SECONDS,
                "deadline {deadline} must stay within now + TTL (+slack) {}",
                now + TTL_SECONDS + SLACK_SECONDS
            );
            repo_ids.push(slot.repo_id);
        }
        let mut ordered = repo_ids.clone();
        ordered.sort();
        ordered.dedup();
        assert_eq!(
            repo_ids, ordered,
            "repo_ids must be unique and strictly ascending (GC-CM-15)"
        );

        // slots.env is the bash-importable view; it must match the sorted slots.
        let env = std::fs::read_to_string(slots_dir.join("slots.env"))
            .expect("provisioner must write slots.env");
        for (index, repo_id) in repo_ids.iter().enumerate() {
            assert!(
                env.contains(&format!(
                    "export LIBRA_CLOUD_LIVE_SLOT_{index}_REPO_ID={repo_id}\n"
                )),
                "slots.env must export slot {index} as `{repo_id}`:\n{env}"
            );
            assert!(
                env.contains(&format!(
                    "export LIBRA_CLOUD_LIVE_SLOT_{index}_R2_PREFIX={repo_id}/\n"
                )),
                "slots.env must export the matching r2_prefix for slot {index}:\n{env}"
            );
        }

        // Idempotence: a second run must refuse to regenerate existing slots.
        let second = run();
        assert!(
            second.status.success(),
            "re-running the provisioner must succeed: stderr={}",
            String::from_utf8_lossy(&second.stderr)
        );
        assert!(
            String::from_utf8_lossy(&second.stderr).contains("slots already provisioned"),
            "second run must report the existing slots, got stderr={}",
            String::from_utf8_lossy(&second.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(slots_dir.join("slots.json")).expect("re-read slots.json"),
            slots_json,
            "an idempotent re-run must not rewrite slots.json"
        );
    }
}
