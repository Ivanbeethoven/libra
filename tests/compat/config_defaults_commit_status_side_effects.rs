use super::*;

fn assert_preview_has_no_post_commit_side_effects(flag: &str) {
    let fixture = Fixture::new();
    let repo = fixture.path(&format!("preview-{}", flag.trim_start_matches('-')));
    fixture.init_repo(&repo);
    fixture.commit_file(&repo, "tracked.txt", "base\n", "base");

    let libra_dir = repo.join(".libra");
    fs::write(
        libra_dir.join("automations.toml"),
        r#"
        [[rules]]
        id = "real_commit_only"
        trigger = { kind = "vcs", event = "post_commit" }
        action = { kind = "prompt", prompt = "summarize the commit" }
        "#,
    )
    .expect("write post-commit automation");
    fixture.success(&repo, &["config", "rerere.enabled", "true"]);

    let id = "a".repeat(64);
    let rerere = libra_dir.join("rerere");
    let entry = rerere.join(&id);
    fs::create_dir_all(&entry).expect("create rerere entry");
    fs::write(entry.join("preimage"), b"conflicted\n").expect("write rerere preimage");
    let merge_rr = format!("{id}\ttracked.txt\n");
    fs::write(rerere.join("MERGE_RR"), &merge_rr).expect("write MERGE_RR");

    fs::write(repo.join("tracked.txt"), "previewed\n").expect("modify tracked file");
    fixture.success(&repo, &["add", "tracked.txt"]);
    fixture.success(
        &repo,
        &[
            "commit",
            flag,
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
    );

    assert_eq!(
        fs::read_to_string(rerere.join("MERGE_RR")).expect("read MERGE_RR after preview"),
        merge_rr,
        "{flag} must not update rerere tracking"
    );
    assert!(
        !entry.join("postimage").exists(),
        "{flag} must not record a rerere postimage"
    );
    let history = stdout_trim(&fixture.success(&repo, &["automation", "history"]));
    assert_eq!(
        history, "No automation history.",
        "{flag} must not dispatch post_commit automation"
    );
}

#[test]
fn dry_run_skips_rerere_and_post_commit_automation() {
    assert_preview_has_no_post_commit_side_effects("--dry-run");
}

#[test]
fn porcelain_skips_rerere_and_post_commit_automation() {
    assert_preview_has_no_post_commit_side_effects("--porcelain");
}

#[test]
fn non_verbose_preview_does_not_require_scratch_writes() {
    let fixture = Fixture::new();
    let repo = fixture.path("non-verbose-read-only-preview");
    fixture.init_repo(&repo);
    fixture.commit_file(&repo, "tracked.txt", "base\n", "base");
    fs::write(repo.join("tracked.txt"), b"staged\n").expect("modify tracked file");
    fixture.success(&repo, &["add", "tracked.txt"]);
    let tmp = repo.join(".libra/tmp");
    if tmp.is_dir() {
        fs::remove_dir_all(&tmp).expect("remove prior scratch directory");
    }
    fs::write(&tmp, b"scratch path intentionally blocked").expect("block scratch path");

    fixture.success(
        &repo,
        &[
            "commit",
            "--dry-run",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
    );
    assert_eq!(
        fs::read(&tmp).expect("read scratch blocker after preview"),
        b"scratch path intentionally blocked"
    );
}
