use super::*;
fn count_files(dir: &Path) -> usize {
    let mut count = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = fs::read_dir(current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                count += 1;
            }
        }
    }
    count
}

fn repo_with_regular_and_lfs_modifications(fixture: &Fixture) -> PathBuf {
    let repo = fixture.path("dry-run-verbose");
    fixture.init_repo(&repo);
    fixture.commit_file(&repo, "tracked.txt", "base\n", "base");
    fs::write(repo.join(".gitattributes"), "*.bin filter=lfs\n").expect("write attributes");
    fs::write(repo.join("tracked.bin"), "base lfs\n").expect("write LFS fixture");
    fixture.success(&repo, &["add", ".gitattributes", "tracked.bin"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "seed lfs"],
    );
    fs::write(repo.join("tracked.txt"), "modified regular\n").expect("modify regular file");
    fs::write(repo.join("tracked.bin"), "modified lfs\n").expect("modify LFS file");
    repo
}

fn assert_verbose_preview(fixture: &Fixture, repo: &Path, args: &[&str]) {
    let output = fixture.run(repo, args);
    assert_success("libra", args, &output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("+modified regular"), "{stderr}");
    assert!(stderr.contains("+oid sha256:"), "{stderr}");
}

#[test]
fn dry_run_auto_stage_verbose_reads_ephemeral_regular_and_lfs_blobs() {
    let fixture = Fixture::new();
    let repo = repo_with_regular_and_lfs_modifications(&fixture);
    let index_path = repo.join(".libra").join("index");
    let index_before = fs::read(&index_path).expect("read live index");
    let objects_before = count_files(&repo.join(".libra").join("objects"));
    let lfs_before = count_files(&repo.join(".libra").join("lfs").join("objects"));

    assert_verbose_preview(
        &fixture,
        &repo,
        &[
            "commit",
            "--dry-run",
            "-a",
            "--no-status",
            "--no-gpg-sign",
            "--no-verify",
            "-v",
            "-m",
            "preview",
        ],
    );
    fixture.success(&repo, &["config", "commit.verbose", "true"]);
    assert_verbose_preview(
        &fixture,
        &repo,
        &[
            "commit",
            "--dry-run",
            "-a",
            "--no-status",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "configured preview",
        ],
    );

    assert_eq!(fs::read(index_path).expect("read live index"), index_before);
    assert_eq!(
        count_files(&repo.join(".libra").join("objects")),
        objects_before
    );
    assert_eq!(
        count_files(&repo.join(".libra").join("lfs").join("objects")),
        lfs_before
    );
}
