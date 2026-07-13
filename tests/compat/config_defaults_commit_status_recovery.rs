use super::*;

fn count_files(dir: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                count += 1;
            }
        }
    }
    count
}

fn repo_with_regular_and_lfs_modifications(fixture: &Fixture, name: &str) -> PathBuf {
    let repo = fixture.path(name);
    fixture.init_repo(&repo);
    fixture.commit_file(&repo, "tracked.txt", "base\n", "base");
    fs::write(repo.join(".gitattributes"), "*.bin filter=lfs\n").expect("write LFS attributes");
    fs::write(repo.join("tracked.bin"), "base lfs\n").expect("write base LFS file");
    fixture.success(&repo, &["add", ".gitattributes", "tracked.bin"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "seed lfs"],
    );
    fs::write(repo.join("tracked.txt"), "modified\n").expect("modify tracked file");
    fs::write(repo.join("tracked.bin"), "modified lfs\n").expect("modify LFS file");
    repo
}

#[cfg(unix)]
fn staged_blob(fixture: &Fixture, repo: &Path, path: &str) -> (String, Vec<u8>) {
    let staged = stdout_trim(&fixture.success(repo, &["ls-files", "--stage", path]));
    let (metadata, staged_path) = staged.split_once('\t').expect("stage row has a tab");
    assert_eq!(staged_path, path);
    let fields = metadata.split_whitespace().collect::<Vec<_>>();
    assert_eq!(fields[0], "120000", "auto-stage must preserve symlink mode");
    let oid = fields[1].to_string();
    let blob = fixture.success(repo, &["cat-file", "-p", &oid]).stdout;
    (oid, blob)
}

#[cfg(unix)]
#[test]
fn real_auto_stage_records_dangling_symlink_target_bytes() {
    let fixture = Fixture::new();
    let repo = fixture.path("real-auto-stage-dangling-symlink");
    fixture.init_repo(&repo);
    fs::write(repo.join("companion.txt"), "base\n").expect("write companion base");
    std::os::unix::fs::symlink("existing-target", repo.join("link"))
        .expect("create initial symlink");
    fixture.success(&repo, &["add", "companion.txt", "link"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "base"],
    );

    fs::remove_file(repo.join("link")).expect("remove initial symlink");
    std::os::unix::fs::symlink("missing-target-after-auto-stage", repo.join("link"))
        .expect("create dangling symlink");
    fs::write(repo.join("companion.txt"), "changed\n").expect("modify companion");
    fixture.success(
        &repo,
        &[
            "commit",
            "-a",
            "--no-status",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "auto-stage dangling symlink",
        ],
    );

    let (_, blob) = staged_blob(&fixture, &repo, "link");
    assert_eq!(blob, b"missing-target-after-auto-stage");
}

#[cfg(unix)]
#[test]
fn real_auto_stage_lfs_pattern_symlink_stays_a_symlink_blob() {
    let fixture = Fixture::new();
    let repo = fixture.path("real-auto-stage-lfs-pattern-symlink");
    fixture.init_repo(&repo);
    fs::write(repo.join(".gitattributes"), "*.bin filter=lfs\n").expect("write attributes");
    fs::write(
        repo.join("first-target.txt"),
        "first payload must not become the link blob\n",
    )
    .expect("write first target");
    fs::write(
        repo.join("second-target.txt"),
        "second payload must not become the link blob\n",
    )
    .expect("write second target");
    std::os::unix::fs::symlink("first-target.txt", repo.join("tracked.bin"))
        .expect("create initial LFS-pattern symlink");
    fixture.success(&repo, &["add", ".gitattributes", "tracked.bin"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "base"],
    );

    fs::remove_file(repo.join("tracked.bin")).expect("remove initial symlink");
    std::os::unix::fs::symlink("second-target.txt", repo.join("tracked.bin"))
        .expect("replace LFS-pattern symlink");
    fixture.success(
        &repo,
        &[
            "commit",
            "-a",
            "--no-status",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "auto-stage LFS-pattern symlink",
        ],
    );

    let (_, blob) = staged_blob(&fixture, &repo, "tracked.bin");
    assert_eq!(blob, b"second-target.txt");
}

#[cfg(unix)]
#[test]
fn non_verbose_auto_stage_preview_does_not_follow_directory_symlink() {
    let fixture = Fixture::new();
    let repo = fixture.path("non-verbose-auto-stage-directory-symlink");
    fixture.init_repo(&repo);
    fs::create_dir(repo.join("target-dir")).expect("create symlink target directory");
    std::os::unix::fs::symlink("old-target", repo.join("link")).expect("create initial symlink");
    fixture.success(&repo, &["add", "link"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "base"],
    );
    let before = fs::read(repo.join(".libra/index")).expect("read live index");
    fs::remove_file(repo.join("link")).expect("remove initial symlink");
    std::os::unix::fs::symlink("target-dir", repo.join("link"))
        .expect("replace link with directory target");

    fixture.success(
        &repo,
        &[
            "commit",
            "--dry-run",
            "-a",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
    );
    assert_eq!(
        fs::read(repo.join(".libra/index")).expect("read live index after preview"),
        before
    );
}

#[cfg(unix)]
#[test]
fn verbose_auto_stage_preview_renders_symlink_target_not_followed_content() {
    let fixture = Fixture::new();
    let repo = fixture.path("verbose-auto-stage-symlink");
    fixture.init_repo(&repo);
    fs::write(repo.join("old-target"), "old followed payload\n").expect("write old target");
    fs::write(repo.join("new-target"), "new followed payload\n").expect("write new target");
    std::os::unix::fs::symlink("old-target", repo.join("link")).expect("create initial symlink");
    fixture.success(&repo, &["add", "link"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "base"],
    );
    fs::remove_file(repo.join("link")).expect("remove initial symlink");
    std::os::unix::fs::symlink("new-target", repo.join("link")).expect("replace symlink");

    let preview = fixture.success(
        &repo,
        &[
            "commit",
            "--dry-run",
            "-a",
            "--verbose",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
    );
    let rendered = String::from_utf8_lossy(&preview.stderr);
    assert!(rendered.contains("+new-target"), "{rendered}");
    assert!(!rendered.contains("new followed payload"), "{rendered}");
}

#[cfg(unix)]
#[test]
fn dry_run_auto_stage_skips_hook_and_editor_and_preserves_live_state() {
    let fixture = Fixture::new();
    let repo = repo_with_regular_and_lfs_modifications(&fixture, "side-effect-free-dry-run");

    let index_path = repo.join(".libra").join("index");
    let before = fs::read(&index_path).expect("read index before dry-run");
    let objects_before = count_files(&repo.join(".libra").join("objects"));
    let lfs_before = count_files(&repo.join(".libra").join("lfs").join("objects"));
    let hook_sentinel = fixture.path("dry-run-hook-ran");
    let hook = repo.join(".libra").join("hooks").join("pre-commit.sh");
    fs::write(
        &hook,
        format!("#!/bin/sh\ntouch \"{}\"\n", hook_sentinel.display()),
    )
    .expect("write dry-run hook");
    let mut hook_permissions = fs::metadata(&hook)
        .expect("read hook metadata")
        .permissions();
    hook_permissions.set_mode(0o755);
    fs::set_permissions(&hook, hook_permissions).expect("make hook executable");

    let editor_sentinel = fixture.path("dry-run-editor-ran");
    let editor = fixture.path("dry-run-editor.sh");
    fs::write(
        &editor,
        format!(
            "#!/bin/sh\ntouch \"{}\"\nprintf '%s\\n' 'dry run subject' > \"$1\"\n",
            editor_sentinel.display()
        ),
    )
    .expect("write dry-run editor");
    let mut editor_permissions = fs::metadata(&editor)
        .expect("read editor metadata")
        .permissions();
    editor_permissions.set_mode(0o755);
    fs::set_permissions(&editor, editor_permissions).expect("make editor executable");

    let preview = fixture
        .libra_command(&repo, &["commit", "--dry-run", "-a", "--no-gpg-sign"])
        .env("EDITOR", editor)
        .output()
        .expect("run side-effect-free dry-run");
    assert_success("libra", &["commit", "--dry-run", "-a"], &preview);
    assert_eq!(
        fs::read(index_path).expect("read index after dry-run failure"),
        before,
        "dry-run must preserve the raw live index"
    );
    assert_eq!(
        count_files(&repo.join(".libra").join("objects")),
        objects_before,
        "dry-run must not persist auto-stage blobs"
    );
    assert_eq!(
        count_files(&repo.join(".libra").join("lfs").join("objects")),
        lfs_before,
        "dry-run must not persist an LFS backup"
    );
    assert!(
        !hook_sentinel.exists(),
        "dry-run must not launch the pre-commit hook"
    );
    assert!(
        !editor_sentinel.exists(),
        "dry-run must not launch the commit-message editor"
    );
}

#[cfg(unix)]
#[test]
fn dry_run_auto_stage_uses_an_isolated_index_and_persists_no_objects() {
    let fixture = Fixture::new();
    let repo = repo_with_regular_and_lfs_modifications(&fixture, "isolated-dry-run-index");

    let index_path = repo.join(".libra").join("index");
    let before = fs::read(&index_path).expect("read index before dry-run");
    let objects_before = count_files(&repo.join(".libra").join("objects"));
    let lfs_before = count_files(&repo.join(".libra").join("lfs").join("objects"));
    fixture.success(
        &repo,
        &[
            "commit",
            "--dry-run",
            "-a",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
    );

    assert_eq!(
        fs::read(&index_path).expect("read index after isolated dry-run"),
        before,
        "dry-run auto-stage must never replace the live index"
    );
    assert_eq!(
        count_files(&repo.join(".libra").join("objects")),
        objects_before
    );
    assert_eq!(
        count_files(&repo.join(".libra").join("lfs").join("objects")),
        lfs_before
    );
    let status = stdout_trim(&fixture.success(&repo, &["status", "--porcelain"]));
    assert!(
        status.lines().any(|line| line == " M tracked.txt")
            && status.lines().any(|line| line == " M tracked.bin"),
        "live index must remain unchanged after the preview: {status}"
    );
    fixture.success(
        &repo,
        &[
            "commit",
            "-a",
            "--no-status",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "real",
        ],
    );
}

#[cfg(unix)]
#[test]
fn real_auto_stage_status_failure_retains_object_valid_regular_and_lfs_entries() {
    let fixture = Fixture::new();
    let repo = repo_with_regular_and_lfs_modifications(&fixture, "real-status-failure");
    fixture.success(&repo, &["config", "core.bare", "true"]);
    let lfs_source = repo.join("tracked.bin");
    let expected_lfs_oid =
        libra::utils::lfs::calc_lfs_file_hash(&lfs_source).expect("hash modified LFS source");
    let expected_lfs_size = fs::metadata(&lfs_source)
        .expect("read modified LFS metadata")
        .len();
    let corrupt_backup = repo
        .join(".libra/lfs/objects")
        .join(&expected_lfs_oid[..2])
        .join(&expected_lfs_oid[2..4])
        .join(&expected_lfs_oid);
    fs::create_dir_all(corrupt_backup.parent().expect("LFS backup has a parent"))
        .expect("create corrupt LFS backup parent");
    fs::write(&corrupt_backup, b"truncated").expect("seed corrupt LFS backup");
    let objects_before = count_files(&repo.join(".libra").join("objects"));

    let rejected = fixture
        .libra_command(&repo, &["commit", "-a", "--no-gpg-sign"])
        .env("EDITOR", "true")
        .output()
        .expect("run real commit with post-auto-stage status failure");
    assert_eq!(rejected.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(stderr.contains("LBR-REPO-003"), "{stderr}");
    assert!(count_files(&repo.join(".libra").join("objects")) > objects_before);
    assert_eq!(
        libra::utils::lfs::calc_lfs_file_hash(&corrupt_backup).expect("hash repaired LFS backup"),
        expected_lfs_oid
    );
    assert_eq!(
        fs::metadata(&corrupt_backup)
            .expect("read repaired LFS backup metadata")
            .len(),
        expected_lfs_size
    );

    fixture.success(&repo, &["config", "core.bare", "false"]);
    let status = stdout_trim(&fixture.success(&repo, &["status", "--porcelain"]));
    assert!(
        status.lines().any(|line| line == "M  tracked.txt")
            && status.lines().any(|line| line == "M  tracked.bin"),
        "real -a must retain a fully staged, object-valid index: {status}"
    );
    fixture.success(
        &repo,
        &[
            "commit",
            "--no-status",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "resume",
        ],
    );
}
