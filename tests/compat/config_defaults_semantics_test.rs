//! High-impact Git config default guards for plan-20260708 P1-05.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[allow(deprecated)]
use libra::internal::{config::Config, db::get_db_conn_instance_for_path};
use serde_json::Value;
use tempfile::{TempDir, tempdir};

const PATH_ENV: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
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

    fn libra_command(&self, cwd: &Path, args: &[&str]) -> Command {
        let config_home = self.home.join(".config");
        let global_db = self.home.join(".libra").join("config.db");
        let system_db = self.home.join(".libra").join("system-config.db");
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
            .env("LIBRA_CONFIG_SYSTEM_DB", &system_db)
            .env("LIBRA_TEST", "1")
            .env("LANG", "C")
            .env("LC_ALL", "C");
        if let Some(profile_file) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile_file);
        }
        command
    }

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
            .env("USERPROFILE", &git_home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Remote User")
            .env("GIT_AUTHOR_EMAIL", "remote@example.com")
            .env("GIT_COMMITTER_NAME", "Remote User")
            .env("GIT_COMMITTER_EMAIL", "remote@example.com")
            .env("LANG", "C")
            .env("LC_ALL", "C");
        command
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.libra_command(cwd, args).output().expect("spawn libra")
    }

    fn success(&self, cwd: &Path, args: &[&str]) -> Output {
        let output = self.run(cwd, args);
        assert_success("libra", args, &output);
        output
    }

    fn git_success(&self, cwd: &Path, args: &[&str]) -> Output {
        let output = self.git_command(cwd, args).output().expect("spawn git");
        assert_success("git", args, &output);
        output
    }

    fn git_stdout(&self, cwd: &Path, args: &[&str]) -> String {
        stdout_trim(&self.git_success(cwd, args))
    }

    fn init_repo(&self, repo: &Path) {
        self.success(&self.root, &["init", "--vault", "false", path_str(repo)]);
        self.success(repo, &["config", "set", "user.name", "Config Test"]);
        self.success(repo, &["config", "set", "user.email", "config@example.com"]);
    }

    #[allow(deprecated)]
    fn legacy_config(
        &self,
        repo: &Path,
        section: &str,
        subsection: Option<&str>,
        variable: &str,
        value: &str,
    ) {
        let db_path = repo.join(".libra").join("libra.db");
        let runtime = tokio::runtime::Runtime::new().expect("create runtime");
        runtime.block_on(async {
            let conn = get_db_conn_instance_for_path(&db_path)
                .await
                .expect("open repo db");
            Config::insert_with_conn(&conn, section, subsection, variable, value).await;
        });
    }

    fn commit_file(&self, repo: &Path, file: &str, content: &str, message: &str) {
        fs::write(repo.join(file), content).expect("write file");
        self.success(repo, &["add", file]);
        self.success(
            repo,
            &["commit", "--no-gpg-sign", "--no-verify", "-m", message],
        );
    }

    fn remote_fixture(&self, name: &str) -> (PathBuf, PathBuf, String) {
        assert!(
            git_available(),
            "pull config compatibility tests require the git binary"
        );

        let remote_dir = self.path(&format!("{name}-remote.git"));
        let work_dir = self.path(&format!("{name}-work"));
        self.git_success(&self.root, &["init", "--bare", path_str(&remote_dir)]);
        self.git_success(&self.root, &["init", path_str(&work_dir)]);
        self.git_success(&work_dir, &["config", "user.name", "Remote User"]);
        self.git_success(&work_dir, &["config", "user.email", "remote@example.com"]);
        fs::write(work_dir.join("README.md"), "base\n").expect("write remote base");
        self.git_success(&work_dir, &["add", "README.md"]);
        self.git_success(&work_dir, &["commit", "-m", "base"]);
        let branch = self.git_stdout(&work_dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
        self.git_success(
            &work_dir,
            &["remote", "add", "origin", path_str(&remote_dir)],
        );
        self.git_success(
            &work_dir,
            &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
        );
        (remote_dir, work_dir, branch)
    }

    fn push_remote_commit(&self, work_dir: &Path, branch: &str, file: &str, message: &str) {
        fs::write(work_dir.join(file), format!("{message}\n")).expect("write remote update");
        self.git_success(work_dir, &["add", file]);
        self.git_success(work_dir, &["commit", "-m", message]);
        self.git_success(
            work_dir,
            &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
        );
    }

    fn configure_tracking(&self, repo: &Path, remote_dir: &Path, branch: &str) {
        self.success(repo, &["remote", "add", "origin", path_str(remote_dir)]);
        self.success(repo, &["config", "branch.main.remote", "origin"]);
        self.success(
            repo,
            &[
                "config",
                "branch.main.merge",
                &format!("refs/heads/{branch}"),
            ],
        );
    }
}

#[test]
fn init_default_branch_config_sets_initial_head_and_cli_flag_wins() {
    let fixture = Fixture::new();
    fixture.success(
        &fixture.root,
        &["config", "--global", "init.defaultBranch", "trunk"],
    );

    let configured = fixture.path("configured");
    fixture.success(
        &fixture.root,
        &["init", "--vault", "false", path_str(&configured)],
    );
    assert_eq!(
        stdout_trim(&fixture.success(&configured, &["symbolic-ref", "--short", "HEAD"])),
        "trunk"
    );

    let explicit = fixture.path("explicit");
    fixture.success(
        &fixture.root,
        &[
            "init",
            "--vault",
            "false",
            "--initial-branch",
            "topic",
            path_str(&explicit),
        ],
    );
    assert_eq!(
        stdout_trim(&fixture.success(&explicit, &["symbolic-ref", "--short", "HEAD"])),
        "topic"
    );
}

#[test]
fn init_default_branch_local_scope_overrides_global_scope() {
    let fixture = Fixture::new();
    fixture.init_repo(&fixture.root);
    fixture.success(
        &fixture.root,
        &["config", "--global", "init.defaultBranch", "global-trunk"],
    );
    fixture.success(
        &fixture.root,
        &["config", "set", "init.defaultBranch", "local-trunk"],
    );
    let child = fixture.path("local-configured");

    fixture.success(
        &fixture.root,
        &["init", "--vault", "false", path_str(&child)],
    );
    assert_eq!(
        stdout_trim(&fixture.success(&child, &["symbolic-ref", "--short", "HEAD"])),
        "local-trunk"
    );
}

#[test]
fn init_default_branch_legacy_config_row_is_honored() {
    let fixture = Fixture::new();
    fixture.init_repo(&fixture.root);
    fixture.legacy_config(&fixture.root, "INIT", None, "defaultBranch", "legacy-trunk");
    let child = fixture.path("legacy-configured");

    fixture.success(
        &fixture.root,
        &["init", "--vault", "false", path_str(&child)],
    );
    assert_eq!(
        stdout_trim(&fixture.success(&child, &["symbolic-ref", "--short", "HEAD"])),
        "legacy-trunk"
    );
}

#[test]
fn init_default_branch_invalid_config_fails_before_creating_repo() {
    let fixture = Fixture::new();
    fixture.success(
        &fixture.root,
        &["config", "--global", "init.defaultBranch", "bad name"],
    );
    let repo = fixture.path("bad");

    let output = fixture.run(
        &fixture.root,
        &["init", "--vault", "false", path_str(&repo)],
    );

    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CLI-002"), "{stderr}");
    assert!(stderr.contains("bad name"), "{stderr}");
    assert!(!repo.join(".libra").exists());
}

#[test]
fn init_default_branch_uses_system_scope_and_case_insensitive_variable() {
    let fixture = Fixture::new();
    fixture.success(
        &fixture.root,
        &["config", "--system", "init.defaultbranch", "system-trunk"],
    );
    let repo = fixture.path("system-configured");

    fixture.success(
        &fixture.root,
        &["init", "--vault", "false", path_str(&repo)],
    );
    assert_eq!(
        stdout_trim(&fixture.success(&repo, &["symbolic-ref", "--short", "HEAD"])),
        "system-trunk"
    );
}

#[test]
fn init_default_branch_empty_config_fails_before_creating_repo() {
    let fixture = Fixture::new();
    fixture.success(
        &fixture.root,
        &["config", "--global", "init.defaultBranch", ""],
    );
    let repo = fixture.path("empty-default-branch");

    let output = fixture.run(
        &fixture.root,
        &["init", "--vault", "false", path_str(&repo)],
    );

    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CLI-002"), "{stderr}");
    assert!(stderr.contains("init.defaultBranch"), "{stderr}");
    assert!(!repo.join(".libra").exists());
}

#[test]
fn init_default_branch_config_read_failure_is_io_error_before_creating_repo() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.home.join(".libra").join("config.db"))
        .expect("create unreadable config-db directory");
    let repo = fixture.path("config-read-failure");

    let output = fixture.run(
        &fixture.root,
        &["init", "--vault", "false", path_str(&repo)],
    );

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-IO-001"), "{stderr}");
    assert!(stderr.contains("init.defaultBranch"), "{stderr}");
    assert!(!repo.join(".libra").exists());
}

#[cfg(unix)]
#[test]
fn init_default_branch_permission_failure_is_io_error_before_creating_repo() {
    let fixture = Fixture::new();
    let config_dir = fixture.home.join(".libra");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o000))
        .expect("make config dir inaccessible");
    let repo = fixture.path("permission-failure");

    let output = fixture.run(
        &fixture.root,
        &["init", "--vault", "false", path_str(&repo)],
    );

    fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o700))
        .expect("restore config dir permissions");
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-IO-001"), "{stderr}");
    assert!(stderr.contains("init.defaultBranch"), "{stderr}");
    assert!(!repo.join(".libra").exists());
}

#[test]
fn pull_rebase_system_config_changes_advice_and_branch_override_wins() {
    let fixture = Fixture::new();
    let repo = fixture.path("pull-rebase-advice");
    fixture.init_repo(&repo);

    let default = fixture.run(&repo, &["pull"]);
    assert!(
        String::from_utf8_lossy(&default.stderr).contains("merge with"),
        "default pull advice should describe merge:\n{}",
        String::from_utf8_lossy(&default.stderr)
    );

    fixture.success(
        &fixture.root,
        &["config", "--system", "pull.Rebase", "true"],
    );
    let configured = fixture.run(&repo, &["pull"]);
    assert!(
        String::from_utf8_lossy(&configured.stderr).contains("rebase against"),
        "pull.rebase=true should describe rebase:\n{}",
        String::from_utf8_lossy(&configured.stderr)
    );

    fixture.success(&repo, &["config", "branch.main.rebase", "false"]);
    let branch_override = fixture.run(&repo, &["pull"]);
    assert!(
        String::from_utf8_lossy(&branch_override.stderr).contains("merge with"),
        "branch.main.rebase=false should override pull.rebase=true:\n{}",
        String::from_utf8_lossy(&branch_override.stderr)
    );
}

#[test]
fn pull_rebase_config_rebases_and_cli_no_rebase_overrides() {
    let fixture = Fixture::new();
    let (remote_dir, work_dir, branch) = fixture.remote_fixture("rebase-config");
    let repo = fixture.path("rebase-config-local");
    fixture.init_repo(&repo);
    fixture.configure_tracking(&repo, &remote_dir, &branch);
    fixture.success(&repo, &["pull"]);

    fixture.push_remote_commit(&work_dir, &branch, "remote.txt", "remote update");
    fixture.commit_file(&repo, "local.txt", "local change\n", "local update");
    fixture.legacy_config(&repo, "PULL", None, "Rebase", "true");
    fixture.success(&repo, &["pull"]);

    let parents = stdout_trim(&fixture.success(&repo, &["log", "-1", "--format=%P"]));
    assert_eq!(parents.split_whitespace().count(), 1);
    assert_eq!(
        stdout_trim(&fixture.success(&repo, &["log", "-1", "--format=%s"])),
        "local update"
    );

    let (remote_dir, work_dir, branch) = fixture.remote_fixture("rebase-cli");
    let repo = fixture.path("rebase-cli-local");
    fixture.init_repo(&repo);
    fixture.configure_tracking(&repo, &remote_dir, &branch);
    fixture.success(&repo, &["pull"]);
    fixture.push_remote_commit(&work_dir, &branch, "remote.txt", "remote update");
    fixture.commit_file(&repo, "local.txt", "local change\n", "local update");
    fixture.success(&repo, &["config", "pull.rebase", "true"]);
    fixture.success(&repo, &["pull", "--no-rebase"]);

    let parents = stdout_trim(&fixture.success(&repo, &["log", "-1", "--format=%P"]));
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "--no-rebase must override pull.rebase=true"
    );
}

#[test]
fn pull_rebase_invalid_config_is_usage_error() {
    let fixture = Fixture::new();
    let repo = fixture.path("pull-rebase-invalid");
    fixture.init_repo(&repo);
    fixture.success(&repo, &["config", "pull.rebase", "maybe"]);

    let output = fixture.run(&repo, &["pull"]);

    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CLI-002"), "{stderr}");
    assert!(stderr.contains("pull.rebase"), "{stderr}");
    assert!(stderr.contains("maybe"), "{stderr}");
}

#[test]
fn pull_rebase_unsupported_modes_are_reported_explicitly() {
    for mode in ["merges", "interactive", "m", "i"] {
        let fixture = Fixture::new();
        let repo = fixture.path(&format!("pull-rebase-{mode}"));
        fixture.init_repo(&repo);
        fixture.success(&repo, &["config", "pull.rebase", mode]);

        let output = fixture.run(&repo, &["pull"]);

        assert_eq!(output.status.code(), Some(129), "mode={mode}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("LBR-CLI-002"), "mode={mode}: {stderr}");
        assert!(stderr.contains("unsupported"), "mode={mode}: {stderr}");
        assert!(stderr.contains(mode), "mode={mode}: {stderr}");
    }
}

#[test]
fn pull_rebase_empty_config_is_usage_error_before_fetch() {
    let fixture = Fixture::new();
    let repo = fixture.path("pull-rebase-empty");
    fixture.init_repo(&repo);
    fixture.success(&fixture.root, &["config", "--global", "pull.rebase", ""]);

    let output = fixture.run(&repo, &["pull"]);

    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CLI-002"), "{stderr}");
    assert!(stderr.contains("pull.rebase"), "{stderr}");
    assert!(!repo.join(".libra").join("FETCH_HEAD").exists());
}

#[test]
fn pull_branch_rebase_invalid_config_is_usage_error() {
    let fixture = Fixture::new();
    let repo = fixture.path("branch-rebase-invalid");
    fixture.init_repo(&repo);
    fixture.success(&repo, &["config", "branch.main.Rebase", "maybe"]);

    let output = fixture.run(&repo, &["pull"]);

    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CLI-002"), "{stderr}");
    assert!(stderr.contains("branch.main.rebase"), "{stderr}");
    assert!(stderr.contains("maybe"), "{stderr}");
}

#[test]
fn pull_ff_invalid_config_fails_before_fetch() {
    let fixture = Fixture::new();
    let (remote_dir, _work_dir, branch) = fixture.remote_fixture("ff-invalid");
    let repo = fixture.path("ff-invalid-local");
    fixture.init_repo(&repo);
    fixture.configure_tracking(&repo, &remote_dir, &branch);
    fixture.success(&repo, &["config", "pull.FF", "maybe"]);

    let output = fixture.run(&repo, &["pull"]);

    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CLI-002"), "{stderr}");
    assert!(stderr.contains("pull.ff"), "{stderr}");
    assert!(stderr.contains("maybe"), "{stderr}");
    assert!(
        !repo.join(".libra").join("FETCH_HEAD").exists(),
        "invalid pull.ff must fail before fetch writes FETCH_HEAD"
    );
}

#[test]
fn pull_config_read_failure_is_io_error_before_fetch() {
    let fixture = Fixture::new();
    let repo = fixture.path("pull-config-read-failure");
    fixture.init_repo(&repo);
    fs::create_dir_all(fixture.home.join(".libra").join("config.db"))
        .expect("create unreadable config-db directory");

    let output = fixture.run(&repo, &["pull"]);

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-IO-001"), "{stderr}");
    assert!(stderr.contains("branch.main.rebase"), "{stderr}");
    assert!(!repo.join(".libra").join("FETCH_HEAD").exists());
}

#[test]
fn pull_ff_only_config_rejects_diverged_history() {
    let fixture = Fixture::new();
    let (remote_dir, work_dir, branch) = fixture.remote_fixture("ff-only");
    let repo = fixture.path("ff-only-local");
    fixture.init_repo(&repo);
    fixture.configure_tracking(&repo, &remote_dir, &branch);
    fixture.success(&repo, &["pull"]);

    fixture.push_remote_commit(&work_dir, &branch, "remote.txt", "remote update");
    fixture.commit_file(&repo, "local.txt", "local change\n", "local update");
    fixture.success(&repo, &["config", "pull.ff", "only"]);

    let output = fixture.run(&repo, &["--json", "pull"]);

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CONFLICT-002"), "{stderr}");
    assert!(stderr.contains("non-fast-forward"), "{stderr}");
}

#[test]
fn pull_ff_false_config_forces_merge_commit_on_fast_forwardable_update() {
    let fixture = Fixture::new();
    let (remote_dir, work_dir, branch) = fixture.remote_fixture("no-ff");
    let repo = fixture.path("no-ff-local");
    fixture.init_repo(&repo);
    fixture.configure_tracking(&repo, &remote_dir, &branch);
    fixture.success(&repo, &["pull"]);

    fixture.push_remote_commit(&work_dir, &branch, "remote.txt", "remote update");
    fixture.success(&repo, &["config", "pull.ff", "false"]);
    fixture.success(&repo, &["pull"]);

    let parents = stdout_trim(&fixture.success(&repo, &["log", "-1", "--format=%P"]));
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "pull.ff=false should force a two-parent merge commit, got parents: {parents}"
    );
}

#[test]
fn pull_ff_true_config_allows_fast_forward() {
    let fixture = Fixture::new();
    let (remote_dir, work_dir, branch) = fixture.remote_fixture("ff-true");
    let repo = fixture.path("ff-true-local");
    fixture.init_repo(&repo);
    fixture.configure_tracking(&repo, &remote_dir, &branch);
    fixture.success(&repo, &["pull"]);

    fixture.push_remote_commit(&work_dir, &branch, "remote.txt", "remote update");
    fixture.success(&repo, &["config", "pull.FF", "true"]);
    fixture.success(&repo, &["pull"]);

    let parents = stdout_trim(&fixture.success(&repo, &["log", "-1", "--format=%P"]));
    assert_eq!(
        parents.split_whitespace().count(),
        1,
        "pull.ff=true should retain fast-forward behavior"
    );
}

#[test]
fn pull_commit_flag_selects_merge_without_overriding_pull_ff() {
    let fixture = Fixture::new();
    let (remote_dir, work_dir, branch) = fixture.remote_fixture("commit-override");
    let repo = fixture.path("commit-override-local");
    fixture.init_repo(&repo);
    fixture.configure_tracking(&repo, &remote_dir, &branch);
    fixture.success(&repo, &["pull"]);

    fixture.push_remote_commit(&work_dir, &branch, "remote.txt", "remote update");
    fixture.success(&repo, &["config", "pull.ff", "only"]);
    fixture.success(&repo, &["config", "pull.rebase", "true"]);
    fixture.success(&repo, &["pull", "--commit"]);

    let parents = stdout_trim(&fixture.success(&repo, &["log", "-1", "--format=%P"]));
    assert_eq!(
        parents.split_whitespace().count(),
        1,
        "--commit must override configured rebase without overriding pull.ff=only"
    );
}

#[test]
fn pull_cli_ff_flags_override_configured_ff_modes() {
    let fixture = Fixture::new();
    let (remote_dir, work_dir, branch) = fixture.remote_fixture("ff-cli");
    let repo = fixture.path("ff-cli-local");
    fixture.init_repo(&repo);
    fixture.configure_tracking(&repo, &remote_dir, &branch);
    fixture.success(&repo, &["pull"]);

    fixture.push_remote_commit(&work_dir, &branch, "allow-ff.txt", "allow fast-forward");
    fixture.success(&repo, &["config", "pull.ff", "false"]);
    fixture.success(&repo, &["pull", "--ff"]);
    let parents = stdout_trim(&fixture.success(&repo, &["log", "-1", "--format=%P"]));
    assert_eq!(
        parents.split_whitespace().count(),
        1,
        "--ff must override pull.ff=false"
    );

    fixture.push_remote_commit(&work_dir, &branch, "force-merge.txt", "force merge");
    fixture.success(&repo, &["config", "pull.ff", "true"]);
    fixture.success(&repo, &["pull", "--no-ff"]);
    let parents = stdout_trim(&fixture.success(&repo, &["log", "-1", "--format=%P"]));
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "--no-ff must override pull.ff=true"
    );

    fixture.push_remote_commit(&work_dir, &branch, "ff-only.txt", "reject non-fast-forward");
    fixture.success(&repo, &["config", "pull.ff", "false"]);
    let output = fixture.run(&repo, &["pull", "--ff-only"]);
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LBR-CONFLICT-002"), "stderr: {stderr}");
    assert!(stderr.contains("non-fast-forward"), "stderr: {stderr}");
}

#[test]
fn pull_config_selected_rebase_is_present_in_json_output() {
    let fixture = Fixture::new();
    let (remote_dir, work_dir, branch) = fixture.remote_fixture("rebase-json");
    let repo = fixture.path("rebase-json-local");
    fixture.init_repo(&repo);
    fixture.configure_tracking(&repo, &remote_dir, &branch);
    fixture.success(&repo, &["pull"]);

    fixture.push_remote_commit(&work_dir, &branch, "remote.txt", "remote update");
    fixture.commit_file(&repo, "local.txt", "local change\n", "local update");
    fixture.success(&repo, &["config", "pull.rebase", "true"]);
    fixture.success(&repo, &["config", "pull.ff", "not-a-merge-policy"]);

    let output = fixture.success(&repo, &["--json", "pull"]);
    let report: Value = serde_json::from_slice(&output.stdout).expect("valid pull JSON");
    assert!(report["data"]["rebase"].is_object(), "report: {report}");
    assert!(report["data"]["merge"].is_null(), "report: {report}");
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .env_clear()
        .env("PATH", PATH_ENV)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("fixture path is utf8")
}

fn stdout_trim(output: &Output) -> String {
    String::from_utf8(output.stdout.clone())
        .expect("stdout should be utf8")
        .trim()
        .to_string()
}

fn assert_success(program: &str, args: &[&str], output: &Output) {
    assert!(
        output.status.success(),
        "{} {} failed\nstdout:\n{}\nstderr:\n{}",
        program,
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
