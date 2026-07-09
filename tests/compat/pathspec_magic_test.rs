//! Shared pathspec magic compatibility guards for plan-20260708 P1-01.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::{TempDir, tempdir};

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempdir().expect("create tempdir");
        let root = temp.path().to_path_buf();
        let home = root.join("home");
        let repo = root.join("repo");
        fs::create_dir_all(&home).expect("create isolated home");
        fs::create_dir_all(&repo).expect("create repo");
        let fixture = Self {
            _temp: temp,
            root,
            home,
            repo,
        };
        fixture.success(
            &fixture.root,
            &["init", "--vault", "false", repo_str(&fixture.repo)],
        );
        fixture.success(
            &fixture.repo,
            &["config", "set", "user.name", "Pathspec Test"],
        );
        fixture.success(
            &fixture.repo,
            &["config", "set", "user.email", "pathspec@example.com"],
        );
        fixture.write("README.md", "root\n");
        fixture.write("src/main.rs", "NEEDLE main\n");
        fixture.write("src/generated.rs", "NEEDLE generated\n");
        fixture.write("src/Case.TXT", "NEEDLE case\n");
        fixture.write("docs/readme.md", "NEEDLE docs\n");
        fixture.write("literal/[abc].txt", "NEEDLE literal\n");
        fixture.success(
            &fixture.repo,
            &[
                "add",
                "README.md",
                "src/main.rs",
                "src/generated.rs",
                "src/Case.TXT",
                "docs/readme.md",
                "literal/[abc].txt",
            ],
        );
        fixture.success(
            &fixture.repo,
            &["commit", "--no-gpg-sign", "--no-verify", "-m", "base"],
        );
        fixture
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

    fn stdout(&self, cwd: &Path, args: &[&str]) -> String {
        String::from_utf8(self.success(cwd, args).stdout).expect("stdout is utf8")
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.repo.join(path);
        fs::create_dir_all(path.parent().expect("file has parent")).expect("create parent");
        fs::write(path, contents).expect("write fixture file");
    }
}

fn repo_str(path: &Path) -> &str {
    path.to_str().expect("repo path is utf8")
}

#[test]
fn ls_files_honors_shared_pathspec_magic() {
    let fixture = Fixture::new();

    let glob_exclude = fixture.stdout(
        &fixture.repo,
        &["ls-files", ":(glob)src/*.rs", ":(exclude)src/generated.rs"],
    );
    assert_eq!(glob_exclude, "src/main.rs\n");

    let case = fixture.stdout(&fixture.repo, &["ls-files", ":(icase)src/case.txt"]);
    assert_eq!(case, "src/Case.TXT\n");

    let literal = fixture.stdout(&fixture.repo, &["ls-files", ":(literal)literal/[abc].txt"]);
    assert_eq!(literal, "literal/[abc].txt\n");

    let src_dir = fixture.repo.join("src");
    let top = fixture.stdout(&src_dir, &["ls-files", ":(top)README.md"]);
    assert_eq!(top, "README.md\n");

    let relative = fixture.stdout(&src_dir, &["ls-files", "*.rs"]);
    assert_eq!(relative, "src/generated.rs\nsrc/main.rs\n");
}

#[test]
fn grep_honors_shared_pathspec_magic() {
    let fixture = Fixture::new();

    let output = fixture.stdout(
        &fixture.repo,
        &[
            "grep",
            "-n",
            "NEEDLE",
            ":(glob)src/*.rs",
            ":(exclude)src/generated.rs",
        ],
    );
    assert!(
        output.contains("src/main.rs:1:NEEDLE main"),
        "grep output should include main.rs:\n{output}"
    );
    assert!(
        !output.contains("generated.rs"),
        "exclude pathspec should remove generated.rs:\n{output}"
    );

    let case = fixture.stdout(
        &fixture.repo,
        &["grep", "-n", "NEEDLE", ":(icase)src/case.txt"],
    );
    assert_eq!(case, "src/Case.TXT:1:NEEDLE case\n");

    let max_depth = fixture.stdout(
        &fixture.repo,
        &[
            "grep",
            "-n",
            "--max-depth",
            "0",
            "NEEDLE",
            ":(glob)src/*.rs",
            ":(exclude)src/generated.rs",
        ],
    );
    assert_eq!(max_depth, "src/main.rs:1:NEEDLE main\n");

    let icase_max_depth = fixture.stdout(
        &fixture.repo,
        &[
            "grep",
            "-n",
            "--max-depth",
            "0",
            "NEEDLE",
            ":(icase)src/case.txt",
        ],
    );
    assert_eq!(icase_max_depth, "src/Case.TXT:1:NEEDLE case\n");
}

#[test]
fn diff_and_status_honor_shared_pathspec_magic() {
    let fixture = Fixture::new();
    fixture.write("src/main.rs", "NEEDLE main\nchanged\n");
    fixture.write("src/generated.rs", "NEEDLE generated\nchanged\n");
    fixture.write("docs/readme.md", "NEEDLE docs\nchanged\n");

    let diff = fixture.stdout(
        &fixture.repo,
        &[
            "diff",
            "--",
            ":(glob)src/*.rs",
            ":(exclude)src/generated.rs",
        ],
    );
    assert!(
        diff.contains("diff --git a/src/main.rs b/src/main.rs"),
        "diff should include src/main.rs:\n{diff}"
    );
    assert!(
        !diff.contains("generated.rs") && !diff.contains("docs/readme.md"),
        "diff should apply exclude and positive filters:\n{diff}"
    );

    let status = fixture.stdout(
        &fixture.repo,
        &[
            "status",
            "--short",
            ":(glob)src/*.rs",
            ":(exclude)src/generated.rs",
        ],
    );
    assert_eq!(status, " M src/main.rs\n");

    let src_dir = fixture.repo.join("src");
    let relative_status = fixture.stdout(&src_dir, &["status", "--short", "*.rs"]);
    assert_eq!(
        relative_status, " M generated.rs\n M main.rs\n",
        "status pathspecs from a subdirectory should match repo-root paths and render cwd-relative entries"
    );
}

#[test]
fn diff_accepts_magic_pathspecs_without_dashdash() {
    let fixture = Fixture::new();
    fixture.write("README.md", "root\nchanged\n");
    fixture.write("src/Case.TXT", "NEEDLE case\nchanged\n");
    fixture.write("docs/readme.md", "NEEDLE docs\nchanged\n");
    fixture.write("literal/[abc].txt", "NEEDLE literal\nchanged\n");

    let top = fixture.stdout(&fixture.repo, &["diff", "--name-only", ":(top)README.md"]);
    assert_eq!(top, "README.md\n");

    let exclude = fixture.stdout(
        &fixture.repo,
        &["diff", "--name-only", ":(exclude)docs/readme.md"],
    );
    assert!(
        exclude.contains("README.md") && !exclude.contains("docs/readme.md"),
        "exclude magic should be parsed as a pathspec without --:\n{exclude}"
    );

    let case = fixture.stdout(
        &fixture.repo,
        &["diff", "--name-only", ":(icase)src/case.txt"],
    );
    assert_eq!(case, "src/Case.TXT\n");

    let literal = fixture.stdout(
        &fixture.repo,
        &["diff", "--name-only", ":(literal)literal/[abc].txt"],
    );
    assert_eq!(literal, "literal/[abc].txt\n");
}
