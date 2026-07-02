//! Integration tests for `libra auth` (lore.md §1.6): the full token
//! lifecycle — write (stdin only, never argv), read/status, expiry
//! detection, revoke — plus the config-namespace lockdown and the
//! never-echo-the-secret invariant.
//!
//! **Layer:** L1 — deterministic (global store isolated per test repo).

use super::*;

const TOKEN: &str = "s3cr3t-token-value-42";

fn login(p: &Path, host: &str, extra: &[&str]) -> std::process::Output {
    let mut argv = vec!["auth", "login", "--host", host, "--with-token"];
    argv.extend_from_slice(extra);
    run_libra_command_with_stdin(&argv, p, &format!("{TOKEN}\n"))
}

#[test]
fn auth_lifecycle_login_status_logout() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    // login via stdin; the secret never appears in ANY output.
    let out = login(p, "git.example.com", &[]);
    assert_cli_success(&out, "login");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains(TOKEN)
            && !String::from_utf8_lossy(&out.stderr).contains(TOKEN),
        "token never echoed"
    );
    // status: json has the host and state but NO token field anywhere.
    let status = run_libra_command(&["--json", "auth", "status"], p);
    assert_cli_success(&status, "status");
    let text = String::from_utf8_lossy(&status.stdout).to_string();
    assert!(!text.contains(TOKEN), "token never in JSON: {text}");
    let json = parse_json_stdout(&status);
    let tokens = json["data"]["tokens"].as_array().unwrap();
    assert_eq!(tokens.len(), 1, "{json}");
    assert_eq!(tokens[0]["host"].as_str(), Some("git.example.com"));
    assert_eq!(tokens[0]["state"].as_str(), Some("valid"));
    assert!(tokens[0].get("token").is_none(), "no token field: {json}");
    // Scriptable single-host contract.
    let hit = run_libra_command(&["auth", "status", "--host", "git.example.com"], p);
    assert_eq!(hit.status.code(), Some(0));
    let miss = run_libra_command(&["auth", "status", "--host", "other.example.com"], p);
    assert_eq!(miss.status.code(), Some(1));
    // Host normalization: the same scope spelled differently matches.
    let normalized = run_libra_command(&["auth", "status", "--host", "https://GIT.Example.COM"], p);
    assert_eq!(
        normalized.status.code(),
        Some(0),
        "normalized spellings match"
    );
    // Re-login overwrites (single token per scope), then logout removes.
    assert_cli_success(
        &login(p, "git.example.com", &["--username", "bot"]),
        "relogin",
    );
    let logout = run_libra_command(
        &["--json", "auth", "logout", "--host", "git.example.com"],
        p,
    );
    assert_cli_success(&logout, "logout");
    assert_eq!(
        parse_json_stdout(&logout)["data"]["removed"].as_i64(),
        Some(1)
    );
    // Idempotent; clear works with nothing stored.
    let again = run_libra_command(&["auth", "logout", "--host", "git.example.com"], p);
    assert_cli_success(&again, "logout idempotent");
    assert_cli_success(&run_libra_command(&["auth", "clear"], p), "clear empty");
}

#[test]
fn auth_expiry_matrix() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    // Relative expiry that has already lapsed by the time status runs.
    assert_cli_success(&login(p, "h1.example", &["--expires-in", "1s"]), "login 1s");
    std::thread::sleep(std::time::Duration::from_secs(2));
    let status = run_libra_command(&["--json", "auth", "status", "--host", "h1.example"], p);
    assert_eq!(status.status.code(), Some(1), "expired token exits 1");
    let json = parse_json_stdout(&status);
    assert_eq!(json["data"]["tokens"][0]["state"].as_str(), Some("expired"));
    // Refusals: past absolute expiry, bad duration forms, both flags.
    let past = login(p, "h2.example", &["--expires-at", "2020-01-01T00:00:00Z"]);
    assert_eq!(past.status.code(), Some(129), "past expiry refused");
    let bare = login(p, "h2.example", &["--expires-at", "20270101"]);
    assert_eq!(bare.status.code(), Some(129), "bare date refused with hint");
    assert!(
        String::from_utf8_lossy(&bare.stderr).contains("RFC3339"),
        "{}",
        String::from_utf8_lossy(&bare.stderr)
    );
    for bad in ["1h30m", "10x", "d", "99999999999999999999d"] {
        let out = login(p, "h2.example", &["--expires-in", bad]);
        assert_eq!(out.status.code(), Some(129), "{bad} refused");
    }
    let both = login(
        p,
        "h2.example",
        &["--expires-at", "2030-01-01T00:00:00Z", "--expires-in", "1d"],
    );
    assert_eq!(both.status.code(), Some(129), "flags mutually exclusive");
}

#[test]
fn auth_input_rules_and_namespace_lockdown() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    // Non-TTY without --with-token is a hard usage error with the pipe hint.
    let no_tty = run_libra_command(&["auth", "login", "--host", "h.example"], p);
    assert_eq!(no_tty.status.code(), Some(129));
    assert!(
        String::from_utf8_lossy(&no_tty.stderr).contains("--with-token"),
        "{}",
        String::from_utf8_lossy(&no_tty.stderr)
    );
    // There is NO --token flag by design.
    let flag = run_libra_command(&["auth", "login", "--host", "h.example", "--token", "x"], p);
    assert_eq!(
        flag.status.code(),
        Some(129),
        "--token flag refused by clap"
    );
    // Bad hosts refused.
    for bad in [
        "",
        "ssh://h.example",
        "https://u:p@h.example",
        "h.example/path",
    ] {
        let out = login(p, bad, &[]);
        assert_eq!(out.status.code(), Some(129), "{bad:?} refused");
    }
    // Store one token, then verify the config surface cannot see or touch it.
    assert_cli_success(&login(p, "h.example", &[]), "login");
    let list = run_libra_command(&["config", "--global", "--list"], p);
    let listing = String::from_utf8_lossy(&list.stdout).to_string();
    assert!(
        !listing.contains(TOKEN),
        "config list never shows the secret: {listing}"
    );
    // Empty token refused.
    let empty = run_libra_command_with_stdin(
        &["auth", "login", "--host", "h2.example", "--with-token"],
        p,
        "\n",
    );
    assert_eq!(empty.status.code(), Some(129), "empty token refused");
}
