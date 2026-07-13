//! Fetch/remote refspec contracts for plan-20260708 P1-06.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::{TempDir, tempdir};

struct CliFixture {
    _temp: TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl CliFixture {
    fn new() -> Self {
        let temp = tempdir().expect("create tempdir");
        let root = temp.path().to_path_buf();
        let home = root.join("home");
        fs::create_dir_all(&home).expect("create isolated home");
        Self {
            _temp: temp,
            root,
            home,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let config_home = self.home.join(".config");
        let global_db = self.home.join(".libra").join("config.db");
        fs::create_dir_all(&config_home).expect("create isolated config dir");
        let mut command = Command::new(env!("CARGO_BIN_EXE_libra"));
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("LIBRA_CONFIG_GLOBAL_DB", &global_db)
            .env("LIBRA_TEST", "1")
            .env("LANG", "C")
            .env("LC_ALL", "C");
        if let Some(profile_file) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile_file);
        }
        command
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd, args).output().expect("spawn libra")
    }

    fn success(&self, cwd: &Path, args: &[&str]) -> Output {
        let output = self.run(cwd, args);
        assert!(
            output.status.success(),
            "{} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn create_source(&self, name: &str) -> PathBuf {
        let source = self.path(name);
        fs::create_dir_all(&source).expect("create source");
        self.success(&self.root, &["init", source.to_str().expect("utf8 source")]);
        self.success(&source, &["config", "set", "user.name", "Refspec Test"]);
        self.success(
            &source,
            &["config", "set", "user.email", "refspec@example.com"],
        );
        fs::write(source.join("shared.txt"), "base\n").expect("write base");
        self.success(&source, &["add", "shared.txt"]);
        self.success(&source, &["commit", "-s", "-m", "base"]);
        self.success(&source, &["branch", "dev"]);

        self.success(&source, &["switch", "dev"]);
        fs::write(source.join("dev.txt"), "dev\n").expect("write dev");
        self.success(&source, &["add", "dev.txt"]);
        self.success(&source, &["commit", "-s", "-m", "dev"]);

        self.success(&source, &["switch", "main"]);
        fs::write(source.join("main.txt"), "main\n").expect("write main");
        self.success(&source, &["add", "main.txt"]);
        self.success(&source, &["commit", "-s", "-m", "main"]);
        source
    }

    fn create_target(&self, name: &str, remotes: &[(&str, &Path)]) -> PathBuf {
        let target = self.path(name);
        fs::create_dir_all(&target).expect("create target");
        self.success(&self.root, &["init", target.to_str().expect("utf8 target")]);
        for (remote, source) in remotes {
            self.success(
                &target,
                &[
                    "remote",
                    "add",
                    remote,
                    source.to_str().expect("utf8 remote path"),
                ],
            );
        }
        target
    }

    fn ref_oid(&self, repo: &Path, reference: &str) -> Option<String> {
        let output = self.run(repo, &["rev-parse", reference]);
        output.status.success().then(|| {
            String::from_utf8(output.stdout)
                .expect("oid output utf8")
                .trim()
                .to_string()
        })
    }
}

#[test]
fn explicit_src_dst_fetch_updates_only_the_requested_destination() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-explicit");
    let target = fixture.create_target("target-explicit", &[("origin", &source)]);
    let source_main = fixture
        .ref_oid(&source, "main")
        .expect("source main should resolve");

    fixture.success(
        &target,
        &[
            "fetch",
            "origin",
            "refs/heads/main:refs/remotes/origin/pinned",
        ],
    );

    assert_eq!(
        fixture.ref_oid(&target, "refs/remotes/origin/pinned"),
        Some(source_main.clone())
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_none()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/dev")
            .is_none()
    );
    let remote_head = fixture.success(
        &target,
        &[
            "for-each-ref",
            "--format=%(refname)|%(symref)",
            "refs/remotes/origin/HEAD",
        ],
    );
    assert!(
        String::from_utf8_lossy(&remote_head.stdout)
            .contains("refs/remotes/origin/HEAD|refs/remotes/origin/pinned")
    );

    // An up-to-date fetch still records the selected source in FETCH_HEAD.
    fixture.success(
        &target,
        &[
            "fetch",
            "origin",
            "refs/heads/main:refs/remotes/origin/pinned",
        ],
    );
    let fetch_head =
        fs::read_to_string(target.join(".libra/FETCH_HEAD")).expect("FETCH_HEAD should be written");
    assert!(fetch_head.contains(&source_main));
    assert!(fetch_head.contains("branch 'main'"));
    assert!(!fetch_head.contains("branch 'dev'"));
    assert!(
        !target.join(".libra/ORIG_HEAD").exists(),
        "plain fetch must not create ORIG_HEAD"
    );
}

#[test]
fn configured_fetch_refspecs_limit_fetch_and_rename_with_tracking_namespace() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-configured");
    let target = fixture.path("target-configured");
    fs::create_dir_all(&target).expect("create target");
    fixture.success(
        &fixture.root,
        &["init", target.to_str().expect("utf8 target")],
    );
    fixture.success(
        &target,
        &[
            "remote",
            "add",
            "-t",
            "main",
            "origin",
            source.to_str().expect("utf8 source"),
        ],
    );

    fixture.success(&target, &["fetch", "origin"]);
    let main_oid = fixture
        .ref_oid(&target, "refs/remotes/origin/main")
        .expect("configured main ref should exist");
    assert!(
        fixture
            .run(&target, &["reflog", "exists", "refs/remotes/origin/main"])
            .status
            .success()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/dev")
            .is_none()
    );

    fixture.success(&target, &["remote", "rename", "origin", "upstream"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_none()
    );
    assert_eq!(
        fixture.ref_oid(&target, "refs/remotes/upstream/main"),
        Some(main_oid)
    );
    let refspec = fixture.success(
        &target,
        &["config", "get", "--all", "remote.upstream.fetch"],
    );
    assert_eq!(
        String::from_utf8_lossy(&refspec.stdout).trim(),
        "+refs/heads/main:refs/remotes/upstream/main"
    );
    assert!(
        !fixture
            .run(&target, &["config", "get", "remote.origin.url"])
            .status
            .success()
    );

    let refs = fixture.success(
        &target,
        &[
            "for-each-ref",
            "--format=%(refname)|%(symref)",
            "refs/remotes/upstream",
        ],
    );
    let refs = String::from_utf8_lossy(&refs.stdout);
    assert!(refs.contains("refs/remotes/upstream/main"));
    assert!(refs.contains("refs/remotes/upstream/HEAD|refs/remotes/upstream/main"));
    assert!(!refs.contains("refs/remotes/origin/"));
    assert!(
        fixture
            .run(&target, &["reflog", "exists", "refs/remotes/upstream/main"])
            .status
            .success()
    );
    assert!(
        !fixture
            .run(&target, &["reflog", "exists", "refs/remotes/origin/main"])
            .status
            .success()
    );
}

#[test]
fn configured_wildcard_refspec_maps_each_matching_branch() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-wildcard");
    let target = fixture.create_target("target-wildcard", &[("origin", &source)]);
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/mapped/*",
        ],
    );

    fixture.success(&target, &["fetch", "origin"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/mapped/main")
            .is_some()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/mapped/dev")
            .is_some()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_none()
    );
    let remote_head = fixture.success(
        &target,
        &[
            "for-each-ref",
            "--format=%(refname)|%(symref)",
            "refs/remotes/origin/HEAD",
        ],
    );
    assert!(
        String::from_utf8_lossy(&remote_head.stdout)
            .contains("refs/remotes/origin/HEAD|refs/remotes/origin/mapped/main")
    );

    fixture.success(&target, &["fetch", "origin", "--prune"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/mapped/main")
            .is_some(),
        "prune must retain a live custom-mapped main ref"
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/mapped/dev")
            .is_some(),
        "prune must retain a live custom-mapped dev ref"
    );
    fixture.success(
        &target,
        &[
            "fetch",
            "origin",
            "refs/heads/main:refs/remotes/origin/pinned",
            "--prune",
        ],
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/mapped/main")
            .is_some(),
        "explicit prune must preserve configured custom mappings"
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/mapped/dev")
            .is_some(),
        "explicit prune must preserve every live configured mapping"
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/pinned")
            .is_some()
    );

    fixture.success(&target, &["remote", "prune", "origin"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/mapped/main")
            .is_some(),
        "remote prune must use the same configured mapping"
    );
}

#[test]
fn configured_fetch_variable_is_case_insensitive_and_renamed_with_its_remote() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-case");
    let target = fixture.create_target("target-case", &[("origin", &source)]);
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "remote.origin.Fetch",
            "+refs/heads/dev:refs/remotes/origin/only-dev",
        ],
    );

    fixture.success(&target, &["fetch", "origin"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/only-dev")
            .is_some()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_none(),
        "case-insensitive Fetch must prevent the default all-branch fallback"
    );
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "--add",
            "remote.origin.Fetch",
            "+refs/remotes/origin/source:refs/remotes/origin/copied",
        ],
    );

    fixture.success(&target, &["remote", "rename", "origin", "upstream"]);
    let refspecs = fixture.success(
        &target,
        &["config", "get", "--all", "remote.upstream.Fetch"],
    );
    let refspecs = String::from_utf8_lossy(&refspecs.stdout);
    assert!(refspecs.contains("+refs/heads/dev:refs/remotes/upstream/only-dev"));
    assert!(refspecs.contains("+refs/remotes/origin/source:refs/remotes/upstream/copied"));
    assert!(!refspecs.contains("+refs/remotes/upstream/source:"));
}

#[test]
fn remote_rename_does_not_capture_dotted_sibling_namespaces() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-dotted");
    let target = fixture.create_target("target-dotted", &[("corp.prod", &source)]);
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "remote.corp.pushurl",
            source.to_str().expect("utf8 source"),
        ],
    );
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "--plaintext",
            "vault.ssh.corp.identity",
            "corp-key",
        ],
    );
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "--plaintext",
            "vault.ssh.corp.prod.identity",
            "corp-prod-key",
        ],
    );

    fixture.success(&target, &["remote", "rename", "corp", "upstream"]);
    assert_eq!(
        String::from_utf8_lossy(
            &fixture
                .success(&target, &["config", "get", "remote.corp.prod.url"])
                .stdout
        )
        .trim(),
        source.to_str().expect("utf8 source")
    );
    assert_eq!(
        String::from_utf8_lossy(
            &fixture
                .success(&target, &["config", "get", "vault.ssh.corp.prod.identity"],)
                .stdout
        )
        .trim(),
        "corp-prod-key"
    );
    assert_eq!(
        String::from_utf8_lossy(
            &fixture
                .success(&target, &["config", "get", "remote.upstream.pushurl"])
                .stdout
        )
        .trim(),
        source.to_str().expect("utf8 source")
    );
    assert_eq!(
        String::from_utf8_lossy(
            &fixture
                .success(&target, &["config", "get", "vault.ssh.upstream.identity"],)
                .stdout
        )
        .trim(),
        "corp-key"
    );
}

#[test]
fn remote_rename_tracking_namespace_conflict_rolls_back_every_migration() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-rename-rollback");
    let target = fixture.create_target("target-rename-rollback", &[("origin", &source)]);
    fixture.success(&target, &["fetch", "origin"]);
    fixture.success(
        &target,
        &[
            "fetch",
            "origin",
            "refs/heads/dev:refs/remotes/upstream/reserved",
        ],
    );

    let output = fixture.run(&target, &["remote", "rename", "origin", "upstream"]);
    assert_eq!(output.status.code(), Some(128));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("tracking reference namespace for remote 'upstream' already exists")
    );
    assert!(
        fixture
            .run(&target, &["config", "get", "remote.origin.url"])
            .status
            .success()
    );
    assert!(
        !fixture
            .run(&target, &["config", "get", "remote.upstream.url"])
            .status
            .success()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_some()
    );
    let origin_head = fixture.success(
        &target,
        &[
            "for-each-ref",
            "--format=%(refname)|%(symref)",
            "refs/remotes/origin/HEAD",
        ],
    );
    assert!(
        String::from_utf8_lossy(&origin_head.stdout)
            .contains("refs/remotes/origin/HEAD|refs/remotes/origin/main")
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/upstream/reserved")
            .is_some()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/upstream/main")
            .is_none()
    );
    assert!(
        fixture
            .run(&target, &["reflog", "exists", "refs/remotes/origin/main"])
            .status
            .success()
    );
    assert!(
        !fixture
            .run(&target, &["reflog", "exists", "refs/remotes/upstream/main"])
            .status
            .success()
    );
}

#[test]
fn duplicate_identical_configured_refspecs_are_tolerated() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-duplicate");
    let target = fixture.path("target-duplicate");
    fs::create_dir_all(&target).expect("create target");
    fixture.success(
        &fixture.root,
        &["init", target.to_str().expect("utf8 target")],
    );
    fixture.success(
        &target,
        &[
            "remote",
            "add",
            "-t",
            "dev",
            "origin",
            source.to_str().expect("utf8 source"),
        ],
    );
    fixture.success(
        &target,
        &["remote", "set-branches", "--add", "origin", "dev"],
    );
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "--add",
            "remote.origin.fetch",
            "refs/heads/dev:refs/remotes/origin/dev",
        ],
    );

    fixture.success(&target, &["fetch", "origin"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/dev")
            .is_some()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_none()
    );
}

#[test]
fn unsupported_fetch_destination_namespaces_fail_before_writes() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-destination");
    fixture.success(&source, &["branch", "EAD"]);
    let target = fixture.create_target("target-destination", &[("origin", &source)]);

    for destination in ["refs/notes/review", "HEAD", "refs/remotes/origin/HEAD"] {
        let refspec = format!("refs/heads/main:{destination}");
        let output = fixture.run(&target, &["fetch", "origin", &refspec]);
        assert_eq!(output.status.code(), Some(129));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Error-Code: LBR-CLI-002"));
        assert!(stderr.contains("refs/heads/* or refs/remotes/<remote>/*"));
    }
    let wildcard = fixture.run(
        &target,
        &["fetch", "origin", "refs/heads/*:refs/remotes/origin/H*"],
    );
    assert_eq!(wildcard.status.code(), Some(129));
    assert!(String::from_utf8_lossy(&wildcard.stderr).contains("reserved HEAD ref"));
    assert!(
        fixture
            .ref_oid(&target, "refs/heads/refs/notes/review")
            .is_none()
    );
    assert!(!target.join(".libra/FETCH_HEAD").exists());
}

#[test]
fn full_fetch_removes_cached_remote_head_when_default_source_is_unmapped() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-head");
    let target = fixture.create_target("target-head", &[("origin", &source)]);
    fixture.success(&target, &["fetch", "origin"]);
    let initial_head = fixture.success(
        &target,
        &[
            "for-each-ref",
            "--format=%(refname)|%(symref)",
            "refs/remotes/origin/HEAD",
        ],
    );
    assert!(
        String::from_utf8_lossy(&initial_head.stdout)
            .contains("refs/remotes/origin/HEAD|refs/remotes/origin/main")
    );

    fixture.success(&target, &["remote", "set-branches", "origin", "dev"]);
    fixture.success(&target, &["fetch", "origin"]);
    let updated_head = fixture.success(
        &target,
        &[
            "for-each-ref",
            "--format=%(refname)|%(symref)",
            "refs/remotes/origin/HEAD",
        ],
    );
    assert!(
        updated_head.stdout.is_empty(),
        "cached HEAD must not retain an unmapped default source"
    );
}

#[test]
fn fetch_rejects_branch_checked_out_in_another_worktree() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-worktree");
    let target = fixture.create_target("target-worktree", &[("origin", &source)]);
    fixture.success(&target, &["fetch", "origin"]);
    fixture.success(
        &target,
        &["switch", "-c", "seeded", "refs/remotes/origin/main"],
    );
    fixture.success(
        &target,
        &["branch", "protected", "refs/remotes/origin/main"],
    );
    let linked = fixture.path("target-worktree-linked");
    fixture.success(
        &target,
        &["worktree", "add", linked.to_str().expect("utf8 linked")],
    );
    fixture.success(&linked, &["switch", "protected"]);
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "remote.origin.fetch",
            "+refs/heads/main:refs/heads/protected",
        ],
    );

    for args in [
        &["fetch", "origin", "--dry-run"][..],
        &["fetch", "origin"][..],
    ] {
        let output = fixture.run(&target, args);
        assert_eq!(output.status.code(), Some(128));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("checked-out branch 'refs/heads/protected'"));
        assert!(stderr.contains("Error-Code: LBR-CONFLICT-002"));
    }
}

#[test]
fn remote_update_uses_remotes_default() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-default");
    let target = fixture.create_target(
        "target-default",
        &[("origin", &source), ("backup", &source)],
    );
    fixture.success(&target, &["config", "set", "remotes.default", "origin"]);

    fixture.success(&target, &["remote", "update"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_some()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/backup/main")
            .is_none()
    );
}

#[test]
fn remotes_default_can_name_a_remote_group() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-default-group");
    let target = fixture.create_target(
        "target-default-group",
        &[("origin", &source), ("backup", &source)],
    );
    fixture.success(&target, &["config", "set", "remotes.default", "both"]);
    fixture.success(&target, &["config", "set", "remotes.both", "origin backup"]);

    fixture.success(&target, &["remote", "update"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_some()
    );
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/backup/main")
            .is_some()
    );
}

#[test]
fn dry_run_does_not_reject_fast_forward_when_remote_tip_is_not_local() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-dry-run-ff");
    let target = fixture.create_target("target-dry-run-ff", &[("origin", &source)]);
    fixture.success(&target, &["fetch", "origin"]);
    let old_oid = fixture
        .ref_oid(&target, "refs/remotes/origin/main")
        .expect("initial tracking ref");

    fs::write(source.join("later.txt"), "later\n").expect("write later file");
    fixture.success(&source, &["add", "later.txt"]);
    fixture.success(&source, &["commit", "-s", "-m", "later"]);
    let new_oid = fixture
        .ref_oid(&source, "main")
        .expect("advanced source main");

    let dry_run = fixture.success(
        &target,
        &[
            "fetch",
            "origin",
            "refs/heads/main:refs/remotes/origin/main",
            "--dry-run",
            "--porcelain",
        ],
    );
    assert!(
        !String::from_utf8_lossy(&dry_run.stdout).starts_with("+ "),
        "unknown pre-download ancestry must not be reported as forced"
    );
    assert_eq!(
        fixture.ref_oid(&target, "refs/remotes/origin/main"),
        Some(old_oid),
        "dry-run must not update the tracking ref"
    );
    fixture.success(&target, &["fetch", "origin"]);
    assert_eq!(
        fixture.ref_oid(&target, "refs/remotes/origin/main"),
        Some(new_oid)
    );
}

#[test]
fn remote_prune_removes_missing_exact_configured_source() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-prune-exact");
    let target = fixture.path("target-prune-exact");
    fs::create_dir_all(&target).expect("create target");
    fixture.success(
        &fixture.root,
        &["init", target.to_str().expect("utf8 target")],
    );
    fixture.success(
        &target,
        &[
            "remote",
            "add",
            "-t",
            "dev",
            "origin",
            source.to_str().expect("utf8 source"),
        ],
    );
    fixture.success(&target, &["fetch", "origin"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/dev")
            .is_some()
    );

    fixture.success(&source, &["branch", "-D", "dev"]);
    fixture.success(&target, &["remote", "prune", "origin"]);
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/dev")
            .is_none()
    );
}

#[test]
fn wildcard_refspec_skips_peeled_annotated_tag_pseudo_refs() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-peeled");
    fixture.success(&source, &["tag", "-m", "version one", "v1"]);
    let target = fixture.create_target("target-peeled", &[("origin", &source)]);
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "remote.origin.fetch",
            "+refs/*:refs/remotes/origin/all/*",
        ],
    );

    fixture.success(&target, &["fetch", "origin"]);
    let refs = fixture.success(
        &target,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/remotes/origin/all",
        ],
    );
    let refs = String::from_utf8_lossy(&refs.stdout);
    assert!(refs.contains("refs/remotes/origin/all/tags/v1"));
    assert!(!refs.contains("^{}"));
}

#[test]
fn invalid_configured_refspec_fails_before_fetch_side_effects() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-invalid");
    let target = fixture.create_target("target-invalid", &[("origin", &source)]);
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/exact",
        ],
    );

    let output = fixture.run(&target, &["fetch", "origin"]);
    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error-Code: LBR-CLI-002"));
    assert!(stderr.contains("matching optional '*' wildcards"));
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_none()
    );
    assert!(!target.join(".libra/FETCH_HEAD").exists());
}

#[test]
fn multi_ref_update_rolls_back_when_one_destination_is_rejected() {
    let fixture = CliFixture::new();
    let source = fixture.create_source("source-atomic");
    let target = fixture.create_target("target-atomic", &[("origin", &source)]);
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "remote.origin.fetch",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
    );
    fixture.success(
        &target,
        &[
            "config",
            "set",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/dev:refs/heads/main",
        ],
    );

    let output = fixture.run(&target, &["fetch", "origin"]);
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("checked-out branch 'refs/heads/main'"));
    assert!(stderr.contains("Error-Code: LBR-CONFLICT-002"));
    assert!(
        fixture
            .ref_oid(&target, "refs/remotes/origin/main")
            .is_none(),
        "the first ref update must roll back with the rejected second update"
    );
    assert!(!target.join(".libra/FETCH_HEAD").exists());
}

#[test]
fn ls_remote_symref_matches_git_advertised_head_shape() {
    if !Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        eprintln!("skipped: git is not available");
        return;
    }

    let fixture = CliFixture::new();
    let work = fixture.path("git-work");
    let bare = fixture.path("git-bare.git");
    fs::create_dir_all(&work).expect("create git worktree");
    let git = |cwd: &Path, args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("HOME", &fixture.home)
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .output()
            .expect("spawn git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&work, &["init", "-b", "main"]);
    git(&work, &["config", "user.name", "Refspec Test"]);
    git(&work, &["config", "user.email", "refspec@example.com"]);
    fs::write(work.join("file.txt"), "git\n").expect("write git file");
    git(&work, &["add", "file.txt"]);
    git(&work, &["commit", "-m", "initial"]);
    git(
        &fixture.root,
        &[
            "clone",
            "--bare",
            work.to_str().expect("utf8 work"),
            bare.to_str().expect("utf8 bare"),
        ],
    );

    let output = fixture.success(
        &fixture.root,
        &["ls-remote", "--symref", bare.to_str().expect("utf8 bare")],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ref: refs/heads/main\tHEAD"));
}
