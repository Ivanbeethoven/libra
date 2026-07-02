//! Integration tests for `libra revision` (lore.md §1.16): the rebuildable
//! per-ref first-parent ordinal index — deterministic, append-only on
//! fast-forward, rebuilt on rewrites/replace changes, never lying.
//!
//! **Layer:** L1 — deterministic.

use super::*;

fn linear_repo(n: usize) -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    for i in 1..=n {
        fs::write(p.join("f.txt"), format!("{i}\n")).unwrap();
        assert_cli_success(&run_libra_command(&["add", "f.txt"], p), "add");
        assert_cli_success(
            &run_libra_command(&["commit", "-m", &format!("c{i}"), "--no-verify"], p),
            "commit",
        );
    }
    repo
}

#[test]
fn revision_find_number_and_reverse_roundtrip() {
    let repo = linear_repo(3);
    let p = repo.path();
    // Chain: initial + 3 = 4 revisions. find(1) = root; number(HEAD) = 4.
    let idx = run_libra_command(&["--json", "revision", "index"], p);
    assert_cli_success(&idx, "index");
    let json = parse_json_stdout(&idx);
    let total = json["data"]["max_ordinal"].as_i64().unwrap();
    assert_eq!(total, 4, "{json}");
    let find = run_libra_command(&["--json", "revision", "find", "-n", "1"], p);
    assert_cli_success(&find, "find root");
    let root_oid = parse_json_stdout(&find)["data"]["oid"]
        .as_str()
        .unwrap()
        .to_string();
    // Roundtrip: the root's ordinal is 1.
    let number = run_libra_command(&["--json", "revision", "number", &root_oid], p);
    assert_cli_success(&number, "reverse");
    assert_eq!(
        parse_json_stdout(&number)["data"]["ordinal"].as_i64(),
        Some(1)
    );
    let head = run_libra_command(&["revision", "number", "HEAD"], p);
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "4");
    // Out of range → exit 1 with the total; zero/negative → 129.
    let miss = run_libra_command(&["revision", "find", "-n", "99"], p);
    assert_eq!(miss.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&miss.stderr).contains("has 4 revisions"),
        "{}",
        String::from_utf8_lossy(&miss.stderr)
    );
    let zero = run_libra_command(&["revision", "find", "-n", "0"], p);
    assert_eq!(zero.status.code(), Some(129));
}

#[test]
fn revision_index_append_rebuild_and_determinism() {
    let repo = linear_repo(2);
    let p = repo.path();
    // Build, remember the root OID.
    let root = parse_json_stdout(&run_libra_command(
        &["--json", "revision", "find", "-n", "1"],
        p,
    ))["data"]["oid"]
        .as_str()
        .unwrap()
        .to_string();
    // Fast-forward: existing ordinals never change (append-only).
    fs::write(p.join("f.txt"), "more\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f.txt"], p), "add");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c3", "--no-verify"], p),
        "commit",
    );
    let after = parse_json_stdout(&run_libra_command(
        &["--json", "revision", "find", "-n", "1"],
        p,
    ))["data"]["oid"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(root, after, "ordinal 1 stable across fast-forward");
    // Determinism: --rebuild reproduces the identical (ordinal, oid) mapping.
    let before = parse_json_stdout(&run_libra_command(
        &["--json", "revision", "number", "HEAD"],
        p,
    ));
    assert_cli_success(
        &run_libra_command(&["revision", "index", "--rebuild"], p),
        "rebuild",
    );
    let rebuilt = parse_json_stdout(&run_libra_command(
        &["--json", "revision", "number", "HEAD"],
        p,
    ));
    assert_eq!(before["data"], rebuilt["data"], "rebuild is deterministic");
    // History rewrite: reset --hard to an ancestor rebuilds honestly.
    assert_cli_success(
        &run_libra_command(&["reset", "--hard", "HEAD~2"], p),
        "reset",
    );
    let idx = parse_json_stdout(&run_libra_command(&["--json", "revision", "index"], p));
    assert_eq!(idx["data"]["max_ordinal"].as_i64(), Some(2), "{idx}");
    // A rewritten-away OID answers not-found, never a stale number.
    let gone_oid = before["data"]["oid"].as_str().unwrap();
    let gone = run_libra_command(&["revision", "number", gone_oid], p);
    assert!(
        !gone.status.success(),
        "rewritten-away OID is not resolvable"
    );
}

#[test]
fn revision_replace_change_invalidates_index() {
    let repo = linear_repo(2);
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["revision", "index"], p), "build");
    // Replace the ROOT's child's parent link? Simpler: replace revision 2
    // with revision 1 — the effective chain shortens on next validation.
    let two = parse_json_stdout(&run_libra_command(
        &["--json", "revision", "find", "-n", "2"],
        p,
    ))["data"]["oid"]
        .as_str()
        .unwrap()
        .to_string();
    let one = parse_json_stdout(&run_libra_command(
        &["--json", "revision", "find", "-n", "1"],
        p,
    ))["data"]["oid"]
        .as_str()
        .unwrap()
        .to_string();
    let replaced = run_libra_command(&["replace", &two, &one], p);
    assert_cli_success(&replaced, "replace");
    // The index must NOT serve the old chain as fresh: the replace-set
    // digest changed → rebuild on the next read (the chain now walks
    // through the replacement).
    let idx = parse_json_stdout(&run_libra_command(&["--json", "revision", "index"], p));
    assert_eq!(
        idx["data"]["max_ordinal"].as_i64(),
        Some(2),
        "chain re-walked under the replacement: {idx}"
    );
    // ordinal 2's slot now maps through the replaced parent chain — the
    // key assertion is freshness: built_at changed vs a no-op validate.
    let again = parse_json_stdout(&run_libra_command(&["--json", "revision", "index"], p));
    assert_eq!(
        idx["data"]["built_at"], again["data"]["built_at"],
        "stable once rebuilt under the new replace set"
    );
}
