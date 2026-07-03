//! Integration tests for `libra media` — the feature-gated FastCDC LFS media
//! chunking client (lore.md §6). Compiled only under `--features fastcdc`.
//!
//! Verifies the client substrate end-to-end without any server: chunk+store,
//! reassemble+verify (incl. a corrupt-chunk failure that writes no output), a
//! valid `--json` envelope, manifest inspect, and the §6.4 safe fallback of the
//! capability probe against an unreachable endpoint (→ standard LFS).
//!
//! Layer: L1 (tempdir + isolated HOME; the only "network" is a connection to a
//! refused loopback port, which resolves immediately to a no-endpoint fallback).
#![cfg(feature = "fastcdc")]

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn media_bin() -> &'static str {
    env!("CARGO_BIN_EXE_libra")
}

fn run(args: &[&str], cwd: &Path) -> Output {
    let home = cwd.join(".libra-test-home");
    fs::create_dir_all(home.join(".config")).unwrap();
    Command::new(media_bin())
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env(
            "LIBRA_CONFIG_GLOBAL_DB",
            home.join(".libra").join("config.db"),
        )
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .expect("run libra")
}

fn ok(args: &[&str], cwd: &Path) -> Output {
    let out = run(args, cwd);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// A fresh initialized repo with a media file large enough to split into
/// several chunks (so dedup/reassembly is meaningfully exercised).
fn repo_with_media() -> (tempfile::TempDir, String) {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    ok(&["init"], p);
    // ~5 MiB of pseudo-random-but-fixed bytes → multiple content-defined chunks.
    let mut data = Vec::with_capacity(5 * 1024 * 1024);
    let mut x: u64 = 0x0BADC0DE_DEADBEEF;
    while data.len() < 5 * 1024 * 1024 {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        data.push((x >> 32) as u8);
    }
    fs::write(p.join("big.bin"), &data).unwrap();
    (repo, "big.bin".to_string())
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("json stdout")
}

#[test]
fn chunk_store_verify_roundtrip() {
    let (repo, file) = repo_with_media();
    let p = repo.path();

    let out = ok(&["--json", "media", "chunk", &file, "--store"], p);
    let js = json(&out);
    let media_oid = js["data"]["media_oid"].as_str().unwrap().to_string();
    assert_eq!(media_oid.len(), 64, "media_oid is sha256 hex");
    assert!(
        js["data"]["chunk_count"].as_u64().unwrap() > 1,
        "multi-chunk"
    );
    assert_eq!(js["data"]["algorithm"].as_str(), Some("fastcdc-v1"));

    // Manifest + chunk store landed under a private .libra/media sibling of objects/.
    let manifest = p
        .join(".libra")
        .join("media")
        .join("manifests")
        .join(format!("{media_oid}.json"));
    assert!(manifest.exists(), "manifest file persisted");
    assert!(
        p.join(".libra").join("media").join("chunks").exists(),
        "chunk store dir exists"
    );
    // Chunks are NOT in the Git object graph.
    assert!(
        !p.join(".libra").join("objects").join("media").exists(),
        "media must not live under objects/"
    );

    // Reassemble + verify the full media_oid.
    let vout = ok(&["--json", "media", "verify", &file], p);
    assert_eq!(json(&vout)["data"]["verified"].as_bool(), Some(true));

    // Inspect the manifest.
    let iout = ok(
        &["--json", "media", "inspect", manifest.to_str().unwrap()],
        p,
    );
    assert_eq!(
        json(&iout)["data"]["media_oid"].as_str(),
        Some(media_oid.as_str())
    );
    assert_eq!(
        json(&iout)["data"]["hash_algorithm"].as_str(),
        Some("sha256")
    );
}

#[test]
fn verify_fails_cleanly_on_a_corrupt_chunk() {
    let (repo, file) = repo_with_media();
    let p = repo.path();
    ok(&["media", "chunk", &file, "--store"], p);

    // Corrupt one stored chunk by truncating it.
    let chunks_dir = p.join(".libra").join("media").join("chunks");
    let mut a_chunk = None;
    for shard in fs::read_dir(&chunks_dir).unwrap() {
        let shard = shard.unwrap().path();
        if shard.is_dir()
            && let Some(entry) = fs::read_dir(&shard).unwrap().next()
        {
            a_chunk = Some(entry.unwrap().path());
            break;
        }
    }
    fs::write(a_chunk.expect("a stored chunk"), b"tampered").unwrap();

    // Verify now fails (non-zero) — the corrupt chunk is caught on read, and no
    // reassembled output is produced.
    let out = run(&["media", "verify", &file], p);
    assert_ne!(
        out.status.code(),
        Some(0),
        "verify must fail on a corrupt chunk"
    );
}

#[test]
fn probe_unreachable_endpoint_falls_back_to_standard_lfs() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    ok(&["init"], p);
    // A refused loopback port → immediate no-endpoint, no external network.
    ok(
        &["config", "remote.origin.url", "https://127.0.0.1:1/x.git"],
        p,
    );

    let out = ok(&["--json", "media", "probe", "--remote", "origin"], p);
    let js = json(&out);
    assert_eq!(
        js["data"]["chunked"].as_bool(),
        Some(false),
        "must fall back"
    );
    assert_eq!(
        js["data"]["decision"].as_str(),
        Some("standard-lfs (fallback)")
    );
    assert_eq!(
        js["data"]["reason"].as_str(),
        Some("no-capability-endpoint")
    );
}
