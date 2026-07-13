use std::io::{Seek, SeekFrom, Write};

use super::*;

const MIB: u64 = 1024 * 1024;

fn write_sparse(path: &Path, size: u64, marker: u8) {
    let mut file = fs::File::create(path).expect("create sparse preview fixture");
    file.set_len(size).expect("size sparse preview fixture");
    file.seek(SeekFrom::Start(0))
        .expect("seek sparse preview fixture");
    file.write_all(&[marker])
        .expect("mark sparse preview fixture");
}

fn assert_preview_limit_failure(fixture: &Fixture, repo: &Path, args: &[&str], expected: &str) {
    let index = repo.join(".libra/index");
    let index_before = fs::read(&index).expect("read index before rejected preview");
    let objects_before = count_files(&repo.join(".libra/objects"));
    let output = fixture.run(repo, args);
    assert_eq!(output.status.code(), Some(128), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-IO-001"), "{stderr}");
    assert!(stderr.contains(expected), "{stderr}");
    assert!(stderr.contains("rerun without --verbose"), "{stderr}");
    assert_eq!(
        fs::read(index).expect("read index after rejected preview"),
        index_before
    );
    assert_eq!(count_files(&repo.join(".libra/objects")), objects_before);
}

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
#[test]
fn verbose_preview_rejects_oversized_already_staged_blob() {
    let fixture = Fixture::new();
    let repo = fixture.path("staged-preview-object-limit");
    fixture.init_repo(&repo);
    fixture.commit_file(&repo, "tracked.bin", "small\n", "base");
    write_sparse(&repo.join("tracked.bin"), 32 * MIB + 1, 1);
    fixture.success(&repo, &["add", "tracked.bin"]);

    assert_preview_limit_failure(
        &fixture,
        &repo,
        &[
            "commit",
            "--dry-run",
            "-v",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
        "33554432 bytes",
    );
}

#[test]
fn verbose_preview_rejects_oversized_head_blob() {
    let fixture = Fixture::new();
    let repo = fixture.path("head-preview-object-limit");
    fixture.init_repo(&repo);
    write_sparse(&repo.join("tracked.bin"), 32 * MIB + 1, 2);
    fixture.success(&repo, &["add", "tracked.bin"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "large base"],
    );
    fs::write(repo.join("tracked.bin"), b"small replacement\n").expect("replace large base");
    fixture.success(&repo, &["add", "tracked.bin"]);

    assert_preview_limit_failure(
        &fixture,
        &repo,
        &[
            "commit",
            "--dry-run",
            "-v",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
        "33554432 bytes",
    );
}

#[test]
fn verbose_preview_does_not_charge_unchanged_oversized_blob() {
    let fixture = Fixture::new();
    let repo = fixture.path("unchanged-large-preview-object");
    fixture.init_repo(&repo);
    write_sparse(&repo.join("large.bin"), 32 * MIB + 1, 4);
    fs::write(repo.join("small.txt"), b"base\n").expect("write small base");
    fixture.success(&repo, &["add", "large.bin", "small.txt"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "base"],
    );
    fs::write(repo.join("small.txt"), b"changed\n").expect("modify small file");
    fixture.success(&repo, &["add", "small.txt"]);
    let index = repo.join(".libra/index");
    let index_before = fs::read(&index).expect("read index before preview");
    let objects_before = count_files(&repo.join(".libra/objects"));

    let output = fixture.run(
        &repo,
        &[
            "commit",
            "--dry-run",
            "-v",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
    );
    assert_success("libra", &["commit", "--dry-run", "-v"], &output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("+changed"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(index).expect("read index after preview"),
        index_before
    );
    assert_eq!(count_files(&repo.join(".libra/objects")), objects_before);
}

#[test]
fn configured_verbose_preview_rejects_aggregate_staged_payload() {
    let fixture = Fixture::new();
    let repo = fixture.path("staged-preview-aggregate-limit");
    fixture.init_repo(&repo);
    for name in ["one.bin", "two.bin", "three.bin"] {
        fs::write(repo.join(name), b"small\n").expect("write base file");
    }
    fixture.success(&repo, &["add", "one.bin", "two.bin", "three.bin"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "base"],
    );
    for (name, marker) in [("one.bin", 1), ("two.bin", 2), ("three.bin", 3)] {
        write_sparse(&repo.join(name), 24 * MIB, marker);
    }
    fixture.success(&repo, &["add", "one.bin", "two.bin", "three.bin"]);
    fixture.success(&repo, &["config", "commit.verbose", "true"]);

    assert_preview_limit_failure(
        &fixture,
        &repo,
        &[
            "commit",
            "--dry-run",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
        "aggregate cache",
    );
}
