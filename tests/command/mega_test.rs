//! Deterministic HTTP-contract tests for `libra mega`.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use super::*;

const TOKEN: &str = "mega-test-token";

fn read_request(mut stream: &TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut content_length = None;
    loop {
        let read = stream.read(&mut buffer).expect("read HTTP request");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            if content_length.is_none() {
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                content_length = headers.lines().find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                });
            }
            let body_len = bytes.len().saturating_sub(header_end + 4);
            if body_len >= content_length.unwrap_or(0) {
                break;
            }
        }
    }
    String::from_utf8(bytes).expect("request is UTF-8")
}

fn serve_once(response_body: &'static str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let address = listener.local_addr().expect("test server address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept test request");
        let request = read_request(&stream);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write test response");
        request
    });
    (format!("http://{address}"), handle)
}

#[test]
fn mega_issue_list_uses_api_contract_and_json_output() {
    let response = r#"{
        "req_result": true,
        "data": {
            "total": 1,
            "items": [{
                "id": 7,
                "link": "ISSUE-7",
                "title": "Agent checkout fails",
                "status": "open",
                "author": "alice",
                "author_is_bot": false,
                "open_timestamp": 1700000000,
                "closed_at": null,
                "merge_timestamp": null,
                "updated_at": 1700000001,
                "labels": [{"id": 2, "name": "bug", "color": "d73a4a", "description": ""}],
                "assignees": ["bob"],
                "comment_num": 3
            }]
        },
        "err_message": ""
    }"#;
    let (host, server) = serve_once(response);
    let repo = tempdir().expect("temp dir");
    let output = base_libra_command(
        &[
            "--json", "mega", "--host", &host, "issue", "list", "--limit", "2",
        ],
        repo.path(),
    )
    .env("MEGA_TOKEN", TOKEN)
    .output()
    .expect("run libra mega issue list");

    assert_cli_success(&output, "mega issue list");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "mega issue list");
    assert_eq!(json["data"]["total"], 1);
    assert_eq!(json["data"]["items"][0]["link"], "ISSUE-7");

    let request = server.join().expect("join test server");
    assert!(
        request.starts_with("POST /api/v1/issue/list HTTP/1.1"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer mega-test-token"),
        "{request}"
    );
    assert!(request.contains("\"page\":1"), "{request}");
    assert!(request.contains("\"per_page\":2"), "{request}");
}

#[test]
fn mega_auth_status_preserves_user_route_trailing_slash() {
    let response = r#"{
        "req_result": true,
        "data": {
            "campsite_user_id": "user-42",
            "github_login": "alice"
        },
        "err_message": ""
    }"#;
    let (host, server) = serve_once(response);
    let repo = tempdir().expect("temp dir");
    let output = base_libra_command(
        &["--json", "mega", "--host", &host, "auth", "status"],
        repo.path(),
    )
    .env("MEGA_TOKEN", TOKEN)
    .output()
    .expect("run libra mega auth status");

    assert_cli_success(&output, "mega auth status");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "mega auth status");
    assert_eq!(json["data"]["github_login"], "alice");

    let request = server.join().expect("join test server");
    assert!(
        request.starts_with("GET /api/v1/user/ HTTP/1.1"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer mega-test-token"),
        "{request}"
    );
}

#[test]
fn mega_issue_view_requires_credentials_before_network() {
    let repo = tempdir().expect("temp dir");
    let output = run_libra_command(
        &[
            "mega",
            "--host",
            "http://127.0.0.1:9",
            "issue",
            "view",
            "ISSUE-7",
        ],
        repo.path(),
    );
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no Mega access token"), "{stderr}");
    assert!(stderr.contains("libra mega auth login"), "{stderr}");
}

#[test]
fn mega_list_rejects_unbounded_page_size_without_network() {
    let repo = tempdir().expect("temp dir");
    let output = run_libra_command(
        &[
            "mega",
            "--host",
            "http://127.0.0.1:9",
            "cl",
            "list",
            "--limit",
            "101",
        ],
        repo.path(),
    );
    assert_eq!(output.status.code(), Some(129));
    assert!(String::from_utf8_lossy(&output.stderr).contains("between 1 and 100"));
}
