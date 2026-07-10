//! Regression tests for configuration-default boundaries identified during the
//! P1-05a production review.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::{TempDir, tempdir};

const PATH_ENV: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    home: PathBuf,
    system_db: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempdir().expect("create tempdir");
        let root = temp.path().to_path_buf();
        let home = root.join("home");
        fs::create_dir_all(&home).expect("create isolated home");
        let system_db = home.join(".libra").join("system.db");
        Self {
            _temp: temp,
            root,
            home,
            system_db,
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
            .env("PATH", PATH_ENV)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("LIBRA_CONFIG_GLOBAL_DB", &global_db)
            .env("LIBRA_CONFIG_SYSTEM_DB", &self.system_db)
            .env("LIBRA_TEST", "1")
            .env("LANG", "C")
            .env("LC_ALL", "C");
        command
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd, args).output().expect("spawn libra")
    }

    fn success(&self, cwd: &Path, args: &[&str]) -> Output {
        let output = self.run(cwd, args);
        assert_success("libra", args, &output);
        output
    }

    fn init_repo(&self, repo: &Path) {
        self.success(&self.root, &["init", "--vault", "false", path_str(repo)]);
        self.success(repo, &["config", "set", "user.name", "Config Test"]);
        self.success(repo, &["config", "set", "user.email", "config@example.com"]);
    }
}

#[test]
fn encrypted_pull_rebase_default_is_decrypted_before_use() {
    let fixture = Fixture::new();
    fixture.success(
        &fixture.root,
        &[
            "config",
            "set",
            "--global",
            "--encrypt",
            "pull.rebase",
            "true",
        ],
    );
    let repo = fixture.path("encrypted-pull");
    fixture.init_repo(&repo);

    let output = fixture.run(&repo, &["pull"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(128), "stderr: {stderr}");
    assert!(stderr.contains("rebase against"), "stderr: {stderr}");
    assert!(!stderr.contains("bad config value"), "stderr: {stderr}");
}

#[test]
fn encrypted_init_default_branch_is_decrypted_before_use() {
    let fixture = Fixture::new();
    fixture.success(
        &fixture.root,
        &[
            "config",
            "set",
            "--global",
            "--encrypt",
            "init.defaultBranch",
            "trunk",
        ],
    );
    let repo = fixture.path("encrypted-init");

    fixture.success(
        &fixture.root,
        &["init", "--vault", "false", path_str(&repo)],
    );
    let branch = fixture.success(&repo, &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "trunk");
}

#[test]
fn encrypted_local_default_branch_is_decrypted_before_use() {
    let fixture = Fixture::new();
    let anchor = fixture.path("local-config-anchor");
    fixture.init_repo(&anchor);
    fixture.success(
        &anchor,
        &[
            "config",
            "set",
            "--encrypt",
            "init.defaultBranch",
            "local-trunk",
        ],
    );
    let repo = fixture.path("encrypted-local-init");

    fixture.success(&anchor, &["init", "--vault", "false", path_str(&repo)]);
    let branch = fixture.success(&repo, &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(
        String::from_utf8_lossy(&branch.stdout).trim(),
        "local-trunk"
    );
}

#[cfg(unix)]
#[test]
fn unreadable_system_default_is_skipped_for_init() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let system_parent = fixture
        .system_db
        .parent()
        .expect("system database has a parent");
    fs::create_dir_all(system_parent).expect("create system config parent");
    fs::create_dir(&fixture.system_db).expect("create unreadable system database directory");
    fs::set_permissions(&fixture.system_db, fs::Permissions::from_mode(0o000))
        .expect("make system database unreadable");

    let repo = fixture.path("system-unreadable");
    let output = fixture.run(
        &fixture.root,
        &["init", "--vault", "false", path_str(&repo)],
    );

    fs::set_permissions(&fixture.system_db, fs::Permissions::from_mode(0o700))
        .expect("restore system database permissions");
    assert_success("libra", &["init"], &output);
}

#[test]
fn init_from_git_reports_the_source_branch_not_the_default_config() {
    let fixture = Fixture::new();
    fixture.success(
        &fixture.root,
        &["config", "set", "--global", "init.defaultBranch", "trunk"],
    );
    let source = fixture.path("git-source");
    let target = fixture.path("converted");
    git_success(
        &fixture,
        &fixture.root,
        &["init", "-q", "-b", "main", path_str(&source)],
    );
    git_success(&fixture, &source, &["config", "user.name", "Source User"]);
    git_success(
        &fixture,
        &source,
        &["config", "user.email", "source@example.com"],
    );
    fs::write(source.join("README.md"), "source\n").expect("write source file");
    git_success(&fixture, &source, &["add", "README.md"]);
    git_success(&fixture, &source, &["commit", "-qm", "source commit"]);

    let output = fixture.success(
        &fixture.root,
        &[
            "--json",
            "init",
            "--vault",
            "false",
            "--from-git-repository",
            path_str(&source),
            path_str(&target),
        ],
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("valid init JSON");
    assert_eq!(report["data"]["initial_branch"], "main");
    let branch = fixture.success(&target, &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "main");
}

#[test]
fn init_from_detached_git_source_fails_before_creating_target_state() {
    let fixture = Fixture::new();
    let source = fixture.path("detached-git-source");
    let target = fixture.path("detached-converted");
    git_success(
        &fixture,
        &fixture.root,
        &["init", "-q", "-b", "main", path_str(&source)],
    );
    git_success(&fixture, &source, &["config", "user.name", "Source User"]);
    git_success(
        &fixture,
        &source,
        &["config", "user.email", "source@example.com"],
    );
    fs::write(source.join("README.md"), "source\n").expect("write source file");
    git_success(&fixture, &source, &["add", "README.md"]);
    git_success(&fixture, &source, &["commit", "-qm", "source commit"]);
    git_success(&fixture, &source, &["checkout", "--detach", "-q"]);

    let output = fixture.run(
        &fixture.root,
        &[
            "init",
            "--vault",
            "false",
            "--from-git-repository",
            path_str(&source),
            path_str(&target),
        ],
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("source Git HEAD is detached"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !target.join(".libra").exists(),
        "failed conversion must not leave target repository state"
    );
}

#[test]
fn pull_merge_option_combinations_match_git_parser_surface() {
    let fixture = Fixture::new();
    let libra_repo = fixture.path("libra-pull-options");
    fixture.init_repo(&libra_repo);
    let git_repo = fixture.path("git-pull-options");
    git_success(
        &fixture,
        &fixture.root,
        &["init", "-q", "-b", "main", path_str(&git_repo)],
    );

    for options in [
        &["--commit", "--ff"][..],
        &["--commit", "--no-ff"],
        &["--commit", "--ff-only"],
        &["--ff-only", "--no-commit"],
        &["--ff-only", "--squash"],
    ] {
        let mut libra_args = vec!["pull"];
        libra_args.extend_from_slice(options);
        let libra_output = fixture.run(&libra_repo, &libra_args);
        assert_ne!(
            libra_output.status.code(),
            Some(129),
            "libra rejected {options:?} during argument parsing: {}",
            String::from_utf8_lossy(&libra_output.stderr)
        );

        let mut git_args = vec!["pull"];
        git_args.extend_from_slice(options);
        let git_output = fixture
            .git_command(&git_repo, &git_args)
            .output()
            .expect("spawn git pull");
        let git_stderr = String::from_utf8_lossy(&git_output.stderr);
        assert!(
            git_stderr.contains("no tracking information"),
            "git did not accept {options:?} through parsing: {git_stderr}"
        );
    }
}

fn git_success(fixture: &Fixture, cwd: &Path, args: &[&str]) {
    let output = fixture.git_command(cwd, args).output().expect("spawn git");
    assert_success("git", args, &output);
}

impl Fixture {
    fn git_command(&self, cwd: &Path, args: &[&str]) -> Command {
        let git_home = self.home.join("git-home");
        fs::create_dir_all(&git_home).expect("create isolated git home");
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", PATH_ENV)
            .env("HOME", &git_home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LANG", "C")
            .env("LC_ALL", "C");
        command
    }
}

fn assert_success(program: &str, args: &[&str], output: &Output) {
    assert!(
        output.status.success(),
        "{program} {args:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("fixture path is utf8")
}
