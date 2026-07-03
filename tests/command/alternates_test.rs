//! Integration tests for object alternates (lore.md 2.3) — borrowing objects
//! from a shared store, and the airtight deletion-safety of a shared base.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::fs;

use super::{assert_cli_success, parse_cli_error_stderr, run_libra_command};

/// Build a committed repo with `<file>` and return (dir, its blob oid).
fn committed_repo(name_hint: &str) -> (tempfile::TempDir, String) {
    let repo = tempfile::tempdir().expect("repo dir");
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["init"], p), "init");
    assert_cli_success(&run_libra_command(&["config", "user.name", "t"], p), "name");
    assert_cli_success(
        &run_libra_command(&["config", "user.email", "t@t"], p),
        "email",
    );
    let fname = format!("{name_hint}.txt");
    fs::write(p.join(&fname), format!("{name_hint} shared content\n")).unwrap();
    assert_cli_success(&run_libra_command(&["add", &fname], p), "add");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "base", "--no-verify"], p),
        "commit",
    );
    let ls = run_libra_command(&["ls-tree", "HEAD"], p);
    let oid = String::from_utf8_lossy(&ls.stdout)
        .lines()
        .find(|l| l.contains(&fname))
        .and_then(|l| {
            l.split_whitespace()
                .find(|w| w.len() == 40 || w.len() == 64)
        })
        .expect("blob oid")
        .to_string();
    (repo, oid)
}

fn objects_dir(repo: &std::path::Path) -> String {
    repo.join(".libra/objects").to_string_lossy().into_owned()
}

#[test]
fn borrower_reads_base_objects_without_a_copy() {
    let (base, oid) = committed_repo("base");
    let borrower = tempfile::tempdir().expect("borrower");
    let bp = borrower.path();
    assert_cli_success(&run_libra_command(&["init"], bp), "init borrower");

    // Before borrowing, the borrower cannot read the base's object.
    let miss = run_libra_command(&["cat-file", "-p", &oid], bp);
    assert_ne!(miss.status.code(), Some(0), "not borrowable yet");

    // Register the alternate; now the borrower reads the base's object.
    assert_cli_success(
        &run_libra_command(&["alternates", "add", &objects_dir(base.path())], bp),
        "add alternate",
    );
    let hit = run_libra_command(&["cat-file", "-p", &oid], bp);
    assert_cli_success(&hit, "borrowed read");
    assert!(String::from_utf8_lossy(&hit.stdout).contains("base shared content"));

    // The borrower's own objects dir does NOT contain the borrowed loose object
    // (read-only borrow, no copy).
    let loose = bp.join(".libra/objects").join(&oid[..2]).join(&oid[2..]);
    assert!(
        !loose.exists(),
        "borrowed object is NOT copied into the borrower"
    );

    // `alternates list` shows it (JSON).
    let list = run_libra_command(&["--json", "alternates", "list"], bp);
    let js: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(
        js["data"]["alternates"].as_array().map(|a| a.len()),
        Some(1)
    );
}

#[test]
fn shared_base_gc_refuses_to_prune_then_allows_after_remove() {
    let (base, _oid) = committed_repo("base");
    let borrower = tempfile::tempdir().expect("borrower");
    let bp = borrower.path();
    assert_cli_success(&run_libra_command(&["init"], bp), "init");
    assert_cli_success(
        &run_libra_command(&["alternates", "add", &objects_dir(base.path())], bp),
        "add alternate",
    );

    // The base now has a live borrower → gc refuses to prune loose objects.
    let gc = run_libra_command(&["maintenance", "run", "--task", "gc"], base.path());
    assert_cli_success(&gc, "gc runs");
    assert!(
        String::from_utf8_lossy(&gc.stdout).contains("shared"),
        "gc skips prune on a shared base: {}",
        String::from_utf8_lossy(&gc.stdout)
    );

    // After the borrower removes the alternate, the base prunes normally.
    assert_cli_success(
        &run_libra_command(&["alternates", "remove", &objects_dir(base.path())], bp),
        "remove alternate",
    );
    let gc2 = run_libra_command(&["maintenance", "run", "--task", "gc"], base.path());
    assert_cli_success(&gc2, "gc after remove");
    assert!(
        !String::from_utf8_lossy(&gc2.stdout).contains("shared"),
        "gc no longer skips: {}",
        String::from_utf8_lossy(&gc2.stdout)
    );
}

#[test]
fn add_refuses_self_reference() {
    let repo = tempfile::tempdir().expect("repo");
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["init"], p), "init");
    let out = run_libra_command(&["alternates", "add", &objects_dir(p)], p);
    assert_ne!(out.status.code(), Some(0), "self-borrow refused");
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

#[test]
fn fsck_reports_dangling_alternate() {
    let repo = tempfile::tempdir().expect("repo");
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["init"], p), "init");
    // Register a base, then delete it → dangling alternate.
    let (base, _oid) = committed_repo("base");
    assert_cli_success(
        &run_libra_command(&["alternates", "add", &objects_dir(base.path())], p),
        "add",
    );
    drop(base); // the base repo (and its objects dir) is removed
    let fsck = run_libra_command(&["fsck"], p);
    assert!(
        String::from_utf8_lossy(&fsck.stderr).contains("dangling object alternate"),
        "fsck flags the dangling alternate: {}",
        String::from_utf8_lossy(&fsck.stderr)
    );
    // Codex P1: a dangling alternate must FAIL fsck (non-zero exit).
    assert_ne!(fsck.status.code(), Some(0), "dangling alternate fails fsck");
}

#[test]
fn shared_base_refuses_obliterate() {
    let (base, oid) = committed_repo("base");
    let borrower = tempfile::tempdir().expect("borrower");
    let bp = borrower.path();
    assert_cli_success(&run_libra_command(&["init"], bp), "init");
    assert_cli_success(
        &run_libra_command(&["alternates", "add", &objects_dir(base.path())], bp),
        "add",
    );
    // The base is now shared → `file obliterate` on its object is refused
    // (a borrower may need it) — Codex P1.
    let out = run_libra_command(&["file", "obliterate", &oid, "--yes"], base.path());
    assert_ne!(
        out.status.code(),
        Some(0),
        "obliterate on a shared base refused"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("shared"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
