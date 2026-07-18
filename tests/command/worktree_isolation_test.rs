//! Integration tests for per-worktree HEAD/index/HEAD-reflog isolation
//! (lore.md 2.1).
//!
//! Verifies: a linked worktree gets its own HEAD, index, and HEAD-reflog while
//! sharing the object store + shared branches; a commit/switch in one worktree
//! never moves another's HEAD; the same-branch guard; the linked-worktree
//! sequencer refusal; and `worktree remove` GCs the private rows. A
//! single-worktree repo is unchanged.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::fs;

use super::{assert_cli_success, run_libra_command};

/// A committed repo (a.txt @ c1) with a `feature` branch. Returns its dir.
fn repo_with_feature() -> tempfile::TempDir {
    let repo = tempfile::tempdir().expect("repo");
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["init", "--vault=false"], p), "init");
    assert_cli_success(&run_libra_command(&["config", "user.name", "t"], p), "name");
    assert_cli_success(
        &run_libra_command(&["config", "user.email", "t@t"], p),
        "email",
    );
    fs::write(p.join("a.txt"), "a\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "a.txt"], p), "add");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c1", "--no-verify"], p),
        "commit",
    );
    assert_cli_success(&run_libra_command(&["branch", "feature"], p), "branch");
    repo
}

fn abbrev_head(dir: &std::path::Path) -> String {
    String::from_utf8_lossy(&run_libra_command(&["rev-parse", "--abbrev-ref", "HEAD"], dir).stdout)
        .trim()
        .to_string()
}

#[test]
fn linked_worktree_has_isolated_head_and_index() {
    let repo = repo_with_feature();
    let main = repo.path();
    let parent = tempfile::tempdir().expect("wt parent");
    let wt = parent.path().join("wt");
    assert_cli_success(
        &run_libra_command(&["worktree", "add", wt.to_str().unwrap()], main),
        "worktree add",
    );

    // The new worktree is DETACHED at c1 (its own HEAD), with a real .libra.
    assert_eq!(abbrev_head(&wt), "HEAD", "new worktree is detached");
    assert!(wt.join(".libra/commondir").exists(), "commondir pointer");
    assert!(
        wt.join(".libra/worktree_id").exists(),
        "private worktree id"
    );
    assert!(wt.join(".libra/index").exists(), "private index");
    // db/objects are NOT duplicated into the linked worktree.
    assert!(
        !wt.join(".libra/libra.db").exists(),
        "db is shared, not copied"
    );

    // Switch the worktree to `feature` and commit there.
    assert_cli_success(&run_libra_command(&["switch", "feature"], &wt), "wt switch");
    fs::write(wt.join("b.txt"), "b\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "b.txt"], &wt), "wt add");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c2-in-wt", "--no-verify"], &wt),
        "wt commit",
    );

    // HEAD isolation: main is still on `main`; the wt commit did NOT move it.
    assert_eq!(
        abbrev_head(main),
        "main",
        "main HEAD unmoved by the wt commit"
    );
    assert_eq!(abbrev_head(&wt), "feature", "wt on its own branch");

    // Index isolation: b.txt is not staged/known in the main worktree.
    let main_status = run_libra_command(&["status", "--porcelain"], main);
    assert!(
        !String::from_utf8_lossy(&main_status.stdout).contains("b.txt"),
        "main index does not see the wt's staged file"
    );

    // HEAD-reflog isolation: the wt commit is not in main's HEAD reflog.
    let main_reflog = run_libra_command(&["reflog"], main);
    assert!(
        !String::from_utf8_lossy(&main_reflog.stdout).contains("c2-in-wt"),
        "main HEAD reflog is independent of the wt"
    );

    // Shared object store: main can resolve the branch tip the wt advanced.
    let feat = run_libra_command(&["log", "feature", "--oneline"], main);
    assert!(
        String::from_utf8_lossy(&feat.stdout).contains("c2-in-wt"),
        "objects + shared branch are visible from main"
    );
}

/// `worktree list --porcelain` reports each worktree's OWN HEAD (Part C
/// §C.3.3): the main worktree on a branch, the linked worktree detached at its
/// own commit — never one shared HEAD stamped onto both entries.
#[test]
fn porcelain_reports_per_worktree_head() {
    let repo = repo_with_feature();
    let main = repo.path();
    let parent = tempfile::tempdir().expect("wt parent");
    let wt = parent.path().join("wt");
    assert_cli_success(
        &run_libra_command(&["worktree", "add", wt.to_str().unwrap()], main),
        "worktree add",
    );

    let out = run_libra_command(&["worktree", "list", "--porcelain"], main);
    assert_cli_success(&out, "worktree list --porcelain");
    let text = String::from_utf8_lossy(&out.stdout).to_string();

    // The main worktree entry carries a branch line...
    assert!(
        text.lines().any(|l| l == "branch refs/heads/main"),
        "main entry reports its branch: {text:?}"
    );
    // ...and the linked worktree entry is detached (its own HEAD), so a
    // `detached` line must appear too.
    assert!(
        text.lines().any(|l| l == "detached"),
        "linked worktree entry reports detached HEAD: {text:?}"
    );
    // Two distinct `worktree <path>` entries, each with its own HEAD line.
    let head_lines = text.lines().filter(|l| l.starts_with("HEAD ")).count();
    assert_eq!(
        head_lines, 2,
        "each worktree has its own HEAD line: {text:?}"
    );
}

#[test]
fn same_branch_is_refused_across_worktrees() {
    let repo = repo_with_feature();
    let main = repo.path();
    // main checks out `feature`.
    assert_cli_success(
        &run_libra_command(&["switch", "feature"], main),
        "main->feature",
    );
    let parent = tempfile::tempdir().expect("wt parent");
    let wt = parent.path().join("wt");
    assert_cli_success(
        &run_libra_command(&["worktree", "add", wt.to_str().unwrap()], main),
        "worktree add",
    );
    // The wt cannot switch to `feature` (checked out in main).
    let refused = run_libra_command(&["switch", "feature"], &wt);
    assert_ne!(refused.status.code(), Some(0), "same-branch switch refused");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("already checked out"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    // But it can switch to a free branch.
    assert_cli_success(
        &run_libra_command(&["switch", "main"], &wt),
        "free branch ok",
    );
}

#[test]
fn sequencer_ops_refused_in_linked_worktree() {
    let repo = repo_with_feature();
    let main = repo.path();
    let parent = tempfile::tempdir().expect("wt parent");
    let wt = parent.path().join("wt");
    assert_cli_success(
        &run_libra_command(&["worktree", "add", wt.to_str().unwrap()], main),
        "worktree add",
    );
    for op in ["merge", "rebase", "cherry-pick", "revert"] {
        let out = run_libra_command(&[op, "feature"], &wt);
        assert_ne!(
            out.status.code(),
            Some(0),
            "{op} refused in linked worktree"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("linked worktree"),
            "{op}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // The same op works in the main worktree.
    assert_cli_success(
        &run_libra_command(&["merge", "feature"], main),
        "merge in main",
    );
}

#[test]
fn remove_gcs_private_head_rows() {
    let repo = repo_with_feature();
    let main = repo.path();
    let parent = tempfile::tempdir().expect("wt parent");
    let wt = parent.path().join("wt");
    assert_cli_success(
        &run_libra_command(&["worktree", "add", wt.to_str().unwrap()], main),
        "worktree add",
    );
    let id = fs::read_to_string(wt.join(".libra/worktree_id"))
        .unwrap()
        .trim()
        .to_string();
    assert!(!id.is_empty(), "worktree id present");

    // Remove the worktree (and its dir); its private HEAD row is GC'd.
    assert_cli_success(
        &run_libra_command(
            &["worktree", "remove", wt.to_str().unwrap(), "--delete-dir"],
            main,
        ),
        "worktree remove",
    );
    // Re-adding at the SAME path (same id) starts clean — detached at HEAD,
    // not inheriting a stale HEAD row.
    fs::create_dir_all(&wt).ok();
    assert_cli_success(
        &run_libra_command(&["worktree", "add", wt.to_str().unwrap()], main),
        "re-add worktree",
    );
    assert_eq!(
        abbrev_head(&wt),
        "HEAD",
        "re-added worktree is cleanly detached"
    );
}
