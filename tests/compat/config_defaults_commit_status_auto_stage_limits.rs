use std::io::{Seek, SeekFrom, Write};

use super::*;

const MIB: u64 = 1024 * 1024;

fn write_sparse(path: &Path, size: u64, marker: u8) {
    let mut file = fs::File::create(path).expect("create sparse auto-stage fixture");
    file.set_len(size).expect("size sparse auto-stage fixture");
    file.seek(SeekFrom::Start(0))
        .expect("seek sparse auto-stage fixture");
    file.write_all(&[marker])
        .expect("mark sparse auto-stage fixture");
}

fn assert_auto_stage_limit(fixture: &Fixture, repo: &Path, expected: &str) {
    let index = repo.join(".libra/index");
    let index_before = fs::read(&index).expect("read index before rejected auto-stage preview");
    let output = fixture.run(
        repo,
        &[
            "commit",
            "-a",
            "--dry-run",
            "-v",
            "--no-gpg-sign",
            "--no-verify",
            "-m",
            "preview",
        ],
    );
    assert_eq!(output.status.code(), Some(128), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-IO-001"), "{stderr}");
    assert!(stderr.contains(expected), "{stderr}");
    assert!(stderr.contains("rerun without --verbose"), "{stderr}");
    assert_eq!(
        fs::read(index).expect("read index after rejected auto-stage preview"),
        index_before,
        "a rejected dry-run must not mutate the real index"
    );
}

#[test]
fn verbose_auto_stage_reserves_aggregate_capacity_before_reading() {
    let fixture = Fixture::new();
    let repo = fixture.path("auto-stage-preview-aggregate-limit");
    fixture.init_repo(&repo);
    for name in ["one.bin", "two.bin", "three.bin"] {
        fs::write(repo.join(name), b"base\n").expect("write tracked base file");
    }
    fixture.success(&repo, &["add", "one.bin", "two.bin", "three.bin"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "base"],
    );
    for (name, marker) in [("one.bin", 1), ("two.bin", 2), ("three.bin", 3)] {
        write_sparse(&repo.join(name), 24 * MIB, marker);
    }

    assert_auto_stage_limit(&fixture, &repo, "aggregate cache");
}

#[test]
fn verbose_auto_stage_reserves_object_slots_before_reading() {
    let fixture = Fixture::new();
    let repo = fixture.path("auto-stage-preview-object-count-limit");
    fixture.init_repo(&repo);
    let tracked = repo.join("tracked");
    fs::create_dir(&tracked).expect("create tracked fixture directory");
    for number in 0..=4_096 {
        fs::write(tracked.join(format!("{number:04}.txt")), b"a").expect("write tracked fixture");
    }
    fixture.success(&repo, &["add", "tracked"]);
    fixture.success(
        &repo,
        &["commit", "--no-gpg-sign", "--no-verify", "-m", "base"],
    );
    for number in 0..=4_096 {
        fs::write(
            tracked.join(format!("{number:04}.txt")),
            format!("changed-{number}\n"),
        )
        .expect("modify tracked fixture");
    }

    assert_auto_stage_limit(&fixture, &repo, "object count exceeds 4096");
}
