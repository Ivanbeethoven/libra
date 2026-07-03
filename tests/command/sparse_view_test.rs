//! Integration tests for the read-only sparse view filter (lore.md 2.2).
//!
//! Verifies: ls-files/diff(working-tree) are scoped to the view; status stays
//! HONEST (never filtered) with an advisory; `diff --staged` is NOT filtered;
//! the working tree is never mutated (no D10 materialization); disable/clear
//! restore the full view.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::fs;

use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

/// A repo with src/a.txt, src/gen/g.txt, docs/d.txt, root.txt committed.
fn repo_with_tree() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    fs::create_dir_all(p.join("src/gen")).unwrap();
    fs::create_dir_all(p.join("docs")).unwrap();
    fs::write(p.join("src/a.txt"), "a\n").unwrap();
    fs::write(p.join("src/gen/g.txt"), "g\n").unwrap();
    fs::write(p.join("docs/d.txt"), "d\n").unwrap();
    fs::write(p.join("root.txt"), "r\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "-A"], p), "add");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "tree", "--no-verify"], p),
        "commit",
    );
    repo
}

#[test]
fn sparse_view_scopes_ls_files_with_negation() {
    let repo = repo_with_tree();
    let p = repo.path();
    // No view → all files listed.
    let full = run_libra_command(&["ls-files"], p);
    let full_out = String::from_utf8_lossy(&full.stdout);
    assert!(full_out.contains("docs/d.txt") && full_out.contains("src/gen/g.txt"));

    // View src/** minus src/gen/** → only src/a.txt.
    assert_cli_success(
        &run_libra_command(&["sparse-view", "set", "src/**", "!src/gen/**"], p),
        "set view",
    );
    let scoped = run_libra_command(&["ls-files"], p);
    let out = String::from_utf8_lossy(&scoped.stdout);
    assert!(out.contains("src/a.txt"), "in-view file shown: {out}");
    assert!(
        !out.contains("src/gen/g.txt"),
        "!negation carved it out: {out}"
    );
    assert!(!out.contains("docs/d.txt"), "out-of-view hidden: {out}");
    assert!(!out.contains("root.txt"), "out-of-view hidden: {out}");
}

#[test]
fn status_stays_honest_and_working_tree_untouched() {
    let repo = repo_with_tree();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["sparse-view", "set", "src/**"], p),
        "set",
    );

    // An out-of-view untracked file MUST still appear in status (honest).
    fs::write(p.join("docs/new.txt"), "new\n").unwrap();
    let status = run_libra_command(&["status", "--porcelain"], p);
    let out = String::from_utf8_lossy(&status.stdout);
    assert!(
        out.contains("docs/new.txt"),
        "status must NOT hide out-of-view changes (commit honesty): {out}"
    );
    // Human status carries the advisory.
    let human = run_libra_command(&["status"], p);
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("sparse view is active"),
        "advisory shown"
    );

    // Read-only guarantee: every working-tree file is still on disk.
    for f in ["src/a.txt", "src/gen/g.txt", "docs/d.txt", "root.txt"] {
        assert!(p.join(f).exists(), "{f} not deleted by the sparse view");
    }
}

#[test]
fn diff_worktree_filtered_but_staged_is_not() {
    let repo = repo_with_tree();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["sparse-view", "set", "src/**"], p),
        "set",
    );

    // Modify one in-view and one out-of-view file.
    fs::write(p.join("src/a.txt"), "a2\n").unwrap();
    fs::write(p.join("docs/d.txt"), "d2\n").unwrap();

    // Working-tree diff is SCOPED to the view.
    let wt = run_libra_command(&["diff", "--name-only"], p);
    let wt_out = String::from_utf8_lossy(&wt.stdout);
    assert!(
        wt_out.contains("src/a.txt"),
        "in-view worktree diff shown: {wt_out}"
    );
    assert!(
        !wt_out.contains("docs/d.txt"),
        "out-of-view worktree diff hidden: {wt_out}"
    );

    // `diff --staged` (commit-authoritative) is NEVER filtered.
    assert_cli_success(&run_libra_command(&["add", "-A"], p), "stage");
    let staged = run_libra_command(&["diff", "--staged", "--name-only"], p);
    let staged_out = String::from_utf8_lossy(&staged.stdout);
    assert!(
        staged_out.contains("src/a.txt") && staged_out.contains("docs/d.txt"),
        "staged diff shows ALL staged changes (honest): {staged_out}"
    );
}

#[test]
fn disable_and_clear_restore_full_view() {
    let repo = repo_with_tree();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["sparse-view", "set", "src/**"], p),
        "set",
    );
    assert!(
        !String::from_utf8_lossy(&run_libra_command(&["ls-files"], p).stdout)
            .contains("docs/d.txt")
    );

    // Disable keeps patterns but restores the full listing.
    assert_cli_success(
        &run_libra_command(&["sparse-view", "disable"], p),
        "disable",
    );
    assert!(
        String::from_utf8_lossy(&run_libra_command(&["ls-files"], p).stdout).contains("docs/d.txt")
    );
    let st = run_libra_command(&["--json", "sparse-view", "status"], p);
    let js: serde_json::Value = serde_json::from_slice(&st.stdout).unwrap();
    assert_eq!(js["data"]["enabled"].as_bool(), Some(false));
    assert_eq!(
        js["data"]["pattern_count"].as_u64(),
        Some(1),
        "patterns kept on disable"
    );

    // Clear drops patterns and disables.
    assert_cli_success(&run_libra_command(&["sparse-view", "clear"], p), "clear");
    let after = run_libra_command(&["--json", "sparse-view", "status"], p);
    let js2: serde_json::Value = serde_json::from_slice(&after.stdout).unwrap();
    assert_eq!(js2["data"]["pattern_count"].as_u64(), Some(0));
}
