use super::*;

#[test]
fn real_auto_stage_lfs_persist_failure_is_contextual_and_preserves_index() {
    let fixture = Fixture::new();
    let repo = fixture.path("lfs-persist-failure");
    fixture.init_repo(&repo);
    fixture.commit_file(&repo, "tracked.txt", "base\n", "base");
    fs::write(repo.join(".gitattributes"), "*.bin filter=lfs\n").expect("write attributes");
    fs::write(repo.join("tracked.bin"), b"base lfs\n").expect("write base LFS file");
    fixture.success(&repo, &["add", ".gitattributes", "tracked.bin"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "seed lfs"],
    );
    fs::write(repo.join("tracked.bin"), b"modified lfs\n").expect("modify LFS file");

    let source = repo.join("tracked.bin");
    let oid = libra::utils::lfs::calc_lfs_file_hash(&source).expect("hash LFS source");
    let blocked_target = repo
        .join(".libra/lfs/objects")
        .join(&oid[..2])
        .join(&oid[2..4])
        .join(&oid);
    fs::create_dir_all(&blocked_target).expect("block final backup with a directory");
    let index_path = repo.join(".libra/index");
    let index_before = fs::read(&index_path).expect("read index before failure");

    let rejected = fixture.run(
        &repo,
        &[
            "commit",
            "-a",
            "--no-status",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "must fail",
        ],
    );
    assert_eq!(rejected.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(stderr.contains("LBR-IO-002"), "{stderr}");
    assert!(stderr.contains(&oid), "missing target OID: {stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(
        fs::read(index_path).expect("read index after failure"),
        index_before,
        "failed atomic persist must leave the live index unchanged"
    );
}
