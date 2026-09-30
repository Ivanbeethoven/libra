#!/usr/bin/env bash
#
# cloud_live_prepare.sh — pre-Nextest writer-slot / repo-ID provisioning for the
# plan-20260927 Cloud FIX cards (FC-CM-LIVE-SAFETY / CM-10 / CM-11).
#
# Purpose (GC-CM-12):
#   Before running any `--features test-live-cloud` test binary, assign the
#   writer slots from the ENTRY set and mint the per-repo IDs the tests are
#   allowed to use. Only these pre-registered UUID/repo-ID/R2 prefixes may be
#   touched by the test process; nothing else may be written/cleaned.
#
# Usage:
#   tests/cloud_live_prepare.sh [--slots-dir <dir>] [--count N]
#   Default: --slots-dir "${LIBRA_CLOUD_LIVE_SLOTS_DIR:-$(mktemp -d)}" --count 8
#
# Output (written to the slots dir):
#   slots.env           — export LIBRA_CLOUD_LIVE_SLOT_<i>_REPO_ID / _R2_PREFIX
#                         (repo IDs use the `test-repo-<lowercase-uuid>` shape
#                          per GC-CM-15; R2 prefix is exactly `<repo_id>/`).
#   slots.json          — machine-readable array (for tests honoring the JsonL
#                         manifest convention).
#   manifest.json       — prepare-time run/slot record, schema
#                         `libra-cloud-live-prepare-v1`. This is NOT the GC-CM-15
#                         `libra-cloud-live-manifest-v1` envelope: that one is
#                         HMAC'd with `mac_key_id` and carries the resources /
#                         global_preimage / backup / local_restore /
#                         restore_target_slots fields, so passing this file to
#                         `validate_manifest_schema` is expected to fail.
#
# Env:
#   LIBRA_CLOUD_LIVE_SLOT_TTL_SECONDS — how far in the future the minted slot and
#                                       run deadlines are set (default 3600).
#
# The script is idempotent: re-running with the same --slots-dir refuses to
# overwrite an existing slots.json unless --force is passed.

set -euo pipefail

slots_dir="${LIBRA_CLOUD_LIVE_SLOTS_DIR:-$(mktemp -d)}"
count=8
force=0

while [ $# -gt 0 ]; do
  case "$1" in
    --slots-dir) slots_dir="$2"; shift 2 ;;
    --count) count="$2"; shift 2 ;;
    --force) force=1; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$slots_dir"

if [ -f "$slots_dir/slots.json" ] && [ "$force" -eq 0 ]; then
  echo "slots already provisioned at $slots_dir/slots.json (pass --force to regen)" >&2
  cat "$slots_dir/slots.json"
  exit 0
fi

# The deadline must be in the FUTURE: `slot_is_live` treats an already-passed
# `expires_at` as dead, so minting slots with `now` would hand out slots that are
# expired on arrival and make the GC-CM-15 deadline gate reject every run.
ttl_seconds="${LIBRA_CLOUD_LIVE_SLOT_TTL_SECONDS:-3600}"
expires_utc="$(python3 -c 'import datetime,sys; print((datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(seconds=int(sys.argv[1]))).strftime("%Y-%m-%dT%H:%M:%SZ"))' "$ttl_seconds")"
owner="${LIBRA_CLOUD_LIVE_OWNER:-genedna}"
run_uuid="$(python3 -c 'import uuid; print(uuid.uuid4())')"

python3 - "$slots_dir" "$count" "$expires_utc" "$owner" "$run_uuid" <<'PY'
import json, sys, uuid, os

slots_dir, count, expires_utc, owner, run_uuid = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4], sys.argv[5]
slots = []
for i in range(count):
    slot_id = str(uuid.uuid4())
    repo_id = f"test-repo-{uuid.uuid4()}"
    slots.append({
        "slot_id": slot_id,
        "repo_id": repo_id,
        "repo_name": f"slot-{i}",
        "r2_prefix": f"{repo_id}/",
        "owner": owner,
        "expires_at": expires_utc,
    })

# GC-CM-15 canonical ordering: `validate_writer_slots` rejects any payload whose
# repo_ids are not strictly ascending, and the minted UUIDs are random, so the
# slots must be sorted before they are written. Re-number `repo_name` afterwards
# so the label matches the slot's index in every emitted artifact.
slots.sort(key=lambda s: s["repo_id"])
for i, s in enumerate(slots):
    s["repo_name"] = f"slot-{i}"

manifest = {
    "schema": "libra-cloud-live-prepare-v1",
    "run": {"uuid": run_uuid, "owner": owner, "expires_at": expires_utc},
    "writer_slots": slots,
}

# Env file (bash-importable).
with open(os.path.join(slots_dir, "slots.env"), "w") as f:
    for i, s in enumerate(slots):
        f.write(f"export LIBRA_CLOUD_LIVE_SLOT_{i}_REPO_ID={s['repo_id']}\n")
        f.write(f"export LIBRA_CLOUD_LIVE_SLOT_{i}_R2_PREFIX={s['r2_prefix']}\n")
    f.write("export LIBRA_CLOUD_LIVE_OWNER=" + owner + "\n")
    f.write("export LIBRA_CLOUD_LIVE_RUN_UUID=" + run_uuid + "\n")

with open(os.path.join(slots_dir, "slots.json"), "w") as f:
    json.dump(slots, f, sort_keys=True)

with open(os.path.join(slots_dir, "manifest.json"), "w") as f:
    json.dump(manifest, f, sort_keys=True, separators=(",", ":"))

print(f"provisioned {count} writer slots under {slots_dir}")
print(json.dumps({"slot_ids": [s["slot_id"] for s in slots],
                  "repo_ids": [s["repo_id"] for s in slots]}, sort_keys=True))
PY

echo "SLOTS_DIR=$slots_dir"
echo "SLOT_DEADLINE_UTC=$expires_utc"
