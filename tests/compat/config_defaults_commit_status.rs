use super::*;

#[cfg(unix)]
pub(super) fn write_capturing_editor(fixture: &Fixture, name: &str, capture: &Path) -> PathBuf {
    let editor = fixture.path(name);
    fs::write(
        &editor,
        format!(
            "#!/bin/sh\ncp \"$1\" \"{}\"\nprintf '%s\\n' 'commit status subject' > \"$1\"\n",
            capture.display()
        ),
    )
    .expect("write capturing editor");
    let mut permissions = fs::metadata(&editor)
        .expect("read editor metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&editor, permissions).expect("make editor executable");
    editor
}

#[cfg(unix)]
fn captured_template(
    fixture: &Fixture,
    repo_name: &str,
    scoped_values: &[(&str, &str)],
    flags: &[&str],
) -> String {
    let repo = fixture.path(repo_name);
    fixture.init_repo(&repo);
    for (scope, value) in scoped_values {
        let args = match *scope {
            "local" => vec!["config", "commit.status", *value],
            "global" => vec!["config", "--global", "commit.status", *value],
            "system" => vec!["config", "--system", "commit.status", *value],
            other => panic!("unsupported config scope {other}"),
        };
        fixture.success(&repo, &args);
    }
    fs::write(repo.join("tracked.txt"), "staged\n").expect("write staged file");
    fixture.success(&repo, &["add", "tracked.txt"]);

    let capture = fixture.path(&format!("{repo_name}-template.txt"));
    let editor = write_capturing_editor(fixture, &format!("{repo_name}-editor.sh"), &capture);
    let mut args = vec!["commit", "--no-gpg-sign", "--no-verify"];
    args.extend_from_slice(flags);
    let output = fixture
        .libra_command(&repo, &args)
        .env("EDITOR", &editor)
        .output()
        .expect("run commit with capturing editor");
    assert_success("libra", &args, &output);
    fs::read_to_string(capture).expect("read captured commit template")
}

#[cfg(unix)]
#[test]
fn commit_status_defaults_true_and_obeys_strict_cascade() {
    let default_fixture = Fixture::new();
    let default_template = captured_template(&default_fixture, "default", &[], &[]);
    assert!(
        default_template.contains("tracked.txt"),
        "unset commit.status must default to true:\n{default_template}"
    );

    let system_fixture = Fixture::new();
    let system_false = captured_template(&system_fixture, "system-false", &[("system", "0k")], &[]);
    assert!(!system_false.contains("tracked.txt"), "{system_false}");

    let global_fixture = Fixture::new();
    let global_over_system = captured_template(
        &global_fixture,
        "global-over-system",
        &[("system", "false"), ("global", "2")],
        &[],
    );
    assert!(
        global_over_system.contains("tracked.txt"),
        "{global_over_system}"
    );

    let local_fixture = Fixture::new();
    let local_over_global = captured_template(
        &local_fixture,
        "local-over-global",
        &[("global", "true"), ("local", "false")],
        &[],
    );
    assert!(
        !local_over_global.contains("tracked.txt"),
        "{local_over_global}"
    );
}

#[cfg(unix)]
#[test]
fn commit_status_cli_flags_override_invalid_or_opposite_config() {
    let status_fixture = Fixture::new();
    let forced_status = captured_template(
        &status_fixture,
        "forced-status",
        &[("local", "invalid")],
        &["--status"],
    );
    assert!(forced_status.contains("tracked.txt"), "{forced_status}");

    let no_status_fixture = Fixture::new();
    let forced_no_status = captured_template(
        &no_status_fixture,
        "forced-no-status",
        &[("local", "true")],
        &["--no-status"],
    );
    assert!(
        !forced_no_status.contains("tracked.txt"),
        "{forced_no_status}"
    );
}

#[test]
fn invalid_commit_status_fails_before_auto_stage_or_history_write() {
    let fixture = Fixture::new();
    let repo = fixture.path("invalid-status");
    fixture.init_repo(&repo);
    fixture.commit_file(&repo, "tracked.txt", "base\n", "base");
    fs::write(repo.join("tracked.txt"), "modified\n").expect("modify tracked file");
    fixture.success(&repo, &["config", "commit.status", "sometimes"]);

    let before = stdout_trim(&fixture.success(&repo, &["rev-parse", "HEAD"]));
    let args = [
        "commit",
        "-a",
        "-e",
        "--no-gpg-sign",
        "--no-verify",
        "-m",
        "must not commit",
    ];
    let rejected = fixture
        .libra_command(&repo, &args)
        .env("EDITOR", "true")
        .output()
        .expect("run commit with invalid applicable commit.status");
    assert_eq!(rejected.status.code(), Some(129));
    assert!(rejected.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(stderr.contains("LBR-CLI-002"), "{stderr}");
    assert!(stderr.contains("commit.status"), "{stderr}");
    assert_eq!(
        stdout_trim(&fixture.success(&repo, &["rev-parse", "HEAD"])),
        before,
        "invalid config must not create a commit"
    );
    let status = stdout_trim(&fixture.success(&repo, &["status", "--porcelain"]));
    assert!(
        status.lines().any(|line| line == " M tracked.txt"),
        "-a must not have staged the file before config validation: {status}"
    );
}

#[test]
fn commit_status_config_read_failure_is_io_error_before_auto_stage() {
    let fixture = Fixture::new();
    let repo = fixture.path("status-read-failure");
    fixture.init_repo(&repo);
    fixture.commit_file(&repo, "tracked.txt", "base\n", "base");
    fs::write(repo.join("tracked.txt"), "modified\n").expect("modify tracked file");
    fs::create_dir_all(fixture.home.join(".libra").join("config.db"))
        .expect("replace global config database with a directory");

    let args = [
        "commit",
        "-a",
        "-e",
        "--no-gpg-sign",
        "--no-verify",
        "-m",
        "must not commit",
    ];
    let rejected = fixture
        .libra_command(&repo, &args)
        .env("EDITOR", "true")
        .output()
        .expect("run commit with unreadable applicable commit.status");
    assert_eq!(rejected.status.code(), Some(128));
    assert!(rejected.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(stderr.contains("LBR-IO-001"), "{stderr}");
    assert!(stderr.contains("commit.status"), "{stderr}");

    fs::remove_dir(fixture.home.join(".libra").join("config.db"))
        .expect("remove unreadable global config path");
    let status = stdout_trim(&fixture.success(&repo, &["status", "--porcelain"]));
    assert!(
        status.lines().any(|line| line == " M tracked.txt"),
        "-a must not have staged the file before config read failure: {status}"
    );
}
