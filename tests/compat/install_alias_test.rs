//! IX-01 installer alias contract.
//!
//! The shell scenario drives the complete installer with an isolated HOME and
//! a deterministic downloader, so it covers both a fresh install and the
//! same-version early-return path without contacting the release service.

use std::{path::PathBuf, process::Command};

#[cfg(unix)]
#[test]
fn installer_creates_and_repairs_safe_optional_lba_alias() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = repo.join("tests/compat/install_alias_smoke.sh");
    let output = Command::new("sh")
        .arg(&script)
        .arg(&repo)
        .output()
        .expect("run POSIX install alias smoke script");

    assert!(
        output.status.success(),
        "install alias smoke failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("install alias smoke: ok"),
        "smoke script did not print its completion marker\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(windows)]
#[test]
fn installer_alias_smoke_windows() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = repo.join("tests/compat/install_alias_windows.ps1");
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg("-RepoRoot")
        .arg(&repo)
        .output()
        .expect("run Windows install alias smoke script");

    assert!(
        output.status.success(),
        "Windows install alias smoke failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("install alias Windows smoke: ok"),
        "Windows smoke script did not print its completion marker\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(all(not(unix), not(windows)))]
#[test]
fn installer_alias_smoke_is_unsupported_platform() {
    eprintln!("skipped: installer aliases are covered on Unix and Windows");
}
