//! Integration tests for `libra media` — the feature-gated FastCDC LFS media
//! chunking client (lore.md §6). Compiled only under `--features fastcdc`.
//!
//! Covers local chunk/store/verify, bounded HTTP transfers against loopback
//! fixtures, integrity failures, and ordinary LFS fallback. An ignored live
//! test connects the real Libra client to a FastCDC-capable HTTP server
//! (monoengine) via `MONOENGINE_FASTCDC_READY_FILE`.
//! Layer: L1 by default (temporary directories and local loopback only).
#![cfg(feature = "fastcdc")]

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn media_bin() -> &'static str {
    env!("CARGO_BIN_EXE_libra")
}

fn supported_capabilities(fallback: bool) -> serde_json::Value {
    serde_json::json!({
        "version": "1", "chunked_lfs": true,
        "chunk_algorithms": ["fastcdc-v2020-32k"], "hash_algorithms": ["sha256"],
        "max_chunk_size": 262144, "max_manifest_size": 1048576,
        "supports_batch_exists": true, "supports_range_read": false,
        "supports_standard_lfs_fallback": fallback,
        "batch_exists": true, "range_read": false, "standard_lfs_fallback": fallback,
        "supports_manifest_id_read": true, "manifest_paging": "v1",
        "max_page_entries": 4096, "max_page_bytes": 1048576
    })
}

fn media_ns(repo: &Path) -> std::path::PathBuf {
    repo.join(".libra").join("media").join("fastcdc-v2020-32k")
}

#[tokio::test]
async fn invalid_remote_manifest_or_chunk_preserves_existing_destination() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Json, Router, routing::get};
    use libra::utils::media::{
        chunk_store::MediaChunkStore, manifest::MediaManifest, transfer::MediaClient,
    };

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::write(&source, b"correct media").unwrap();
    let (manifest, _) = MediaManifest::build_from_file(&source).unwrap();
    let real_id = manifest.id().unwrap();
    for wrong_identity in [false, true] {
        let id = if wrong_identity {
            "a".repeat(64)
        } else {
            real_id.clone()
        };
        let returned = serde_json::json!({"manifest_id": id, "manifest": manifest.clone()});
        let summary = serde_json::json!({
            "version": manifest.version,
            "algorithm": manifest.algorithm,
            "hash_algorithm": manifest.hash_algorithm,
            "oid": manifest.media_oid,
            "size": manifest.media_size,
            "chunk_count": manifest.chunks.len(),
            "page_count": 1,
            "manifest_id": real_id.clone(),
            "created_by": manifest.created_by.clone(),
        });
        let pages = serde_json::json!({
            "manifest_id": real_id.clone(),
            "pages": [{
                "page_no": 0,
                "offset_start": 0,
                "offset_end": manifest.media_size,
                "entries": manifest.chunks.clone(),
            }],
        });
        let chunk_requests = Arc::new(AtomicUsize::new(0));
        let counted = chunk_requests.clone();
        let app = Router::new()
            .route(
                "/repo.git/info/lfs/libra/media/v1/capabilities",
                get(|| async { Json(supported_capabilities(true)) }),
            )
            .route(
                "/repo.git/info/lfs/libra/media/v1/manifests/by-media/{oid}",
                get(move || {
                    let response = returned.clone();
                    async move { Json(response) }
                }),
            )
            .route(
                "/repo.git/info/lfs/libra/media/v1/finalized/{id}",
                get({
                    let summary = summary.clone();
                    move || {
                        let summary = summary.clone();
                        async move { Json(summary) }
                    }
                }),
            )
            .route(
                "/repo.git/info/lfs/libra/media/v1/finalized/{id}/pages",
                get({
                    let pages = pages.clone();
                    move || {
                        let pages = pages.clone();
                        async move { Json(pages) }
                    }
                }),
            )
            .route(
                "/repo.git/info/lfs/libra/media/v1/finalized/{id}/chunks/{hash}",
                get(move || {
                    counted.fetch_add(1, Ordering::SeqCst);
                    async { "corrupt media" }
                }),
            )
            .layer(axum::middleware::from_fn(
                |request: axum::extract::Request, next: axum::middleware::Next| async move {
                    assert_eq!(request.uri().query(), Some("tenant=test"));
                    next.run(request).await
                },
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!(
            "http://{}/repo.git/info/lfs/?tenant=test",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = MediaClient::discover(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            &base,
            false,
        )
        .await
        .unwrap()
        .unwrap();
        let dest = dir.path().join("dest");
        fs::write(&dest, b"keep me").unwrap();
        let store = MediaChunkStore::at(dir.path().join("chunks"));
        let error = client
            .download(&manifest.media_oid, manifest.media_size, &dest, &store)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(if wrong_identity {
                "manifest does not match"
            } else {
                "chunk size or SHA-256 mismatch"
            }),
            "{error:#}"
        );
        assert_eq!(
            chunk_requests.load(Ordering::SeqCst),
            usize::from(!wrong_identity)
        );
        assert_eq!(fs::read(dest).unwrap(), b"keep me");
        task.abort();
    }
}

#[tokio::test]
async fn ordinary_lfs_server_falls_back_to_full_transfer() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use axum::{
        Json, Router,
        body::Bytes,
        routing::{get, post, put},
    };
    use libra::{internal::protocol::lfs_client::LFSClient, utils::media::transfer::MediaClient};

    // No extension, chunk-only policy, and an extension whose manifest limit
    // is too small must all retain the standard complete-object path.
    for (advertise, fallback, small_manifest) in [
        (false, true, false),
        (true, false, false),
        (true, true, true),
    ] {
        let data = "plain LFS bytes";
        let oid =
            hex::encode(ring::digest::digest(&ring::digest::SHA256, data.as_bytes()).as_ref());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote = format!("http://{}/repo.git", listener.local_addr().unwrap());
        let base = format!("{remote}/info/lfs/");
        let object_base = format!("{base}objects/");
        let uploaded = Arc::new(AtomicBool::new(false));
        let saved = uploaded.clone();
        let mut app = Router::new()
            .route("/repo.git/info/lfs/objects/batch", post(move |Json(request): Json<serde_json::Value>| {
                let object_base = object_base.clone();
                async move {
                    let action = request["operation"].as_str().unwrap();
                    let requested_oid = request["objects"][0]["oid"].as_str().unwrap();
                    let href = format!("{object_base}{requested_oid}");
                    Json(serde_json::json!({"transfer":"basic","objects":[{
                        "oid":requested_oid, "size":data.len(), "actions":{(action):{"href":href,"expires_at":""}}
                    }]}))
                }
            }))
            .route("/repo.git/info/lfs/objects/{oid}", get(move || async move { data }).merge(put(move |body: Bytes| {
                assert_eq!(body.as_ref(), data.as_bytes());
                saved.store(true, Ordering::SeqCst);
                async { axum::http::StatusCode::OK }
            })));
        if advertise {
            app = app.route(
                "/repo.git/info/lfs/libra/media/v1/capabilities",
                get(move || async move {
                    let mut caps = supported_capabilities(fallback);
                    if small_manifest {
                        caps["max_manifest_size"] = serde_json::json!(32);
                    }
                    Json(caps)
                }),
            );
        }
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut client = LFSClient::from_remote_url(&remote).unwrap();
        client.client = reqwest::Client::builder().no_proxy().build().unwrap();
        // A chunk-only advertisement must not block either basic operation.
        assert!(
            MediaClient::discover(client.client.clone(), &client.lfs_url, false)
                .await
                .unwrap()
                .is_none(),
            "advertise={advertise} fallback={fallback} small={small_manifest} stays on basic LFS"
        );
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::write(&source, data).unwrap();
        assert!(client.push_object(&oid, &source).await.unwrap());
        assert!(uploaded.load(Ordering::SeqCst));
        let dest = dir.path().join("dest");
        client
            .download_object(&oid, data.len() as u64, &dest, None)
            .await
            .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), data.as_bytes());
        // A legal SHA-256 OID for different content reaches the checksum error
        // path. Its replacement pointer must be visible as soon as we return.
        let wrong_oid = hex::encode(
            ring::digest::digest(&ring::digest::SHA256, b"a different LFS object").as_ref(),
        );
        let error = client
            .download_object(&wrong_oid, data.len() as u64, &dest, None)
            .await
            .expect_err("incorrect object bytes must fail checksum verification");
        assert!(error.to_string().contains("Checksum mismatch"), "{error:#}");
        assert_eq!(
            fs::read_to_string(&dest).unwrap(),
            libra::utils::lfs::format_pointer_string(&wrong_oid, data.len() as u64),
            "the checksum error must flush the complete fallback pointer before returning"
        );
        task.abort();
    }
}

#[test]
fn relative_reassembly_target_is_replaced_only_after_verification() {
    use libra::utils::media::{
        chunk_store::{self, MediaChunkStore},
        manifest::MediaManifest,
    };
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::write(&source, b"verified replacement").unwrap();
    let (mut manifest, _) = MediaManifest::build_from_file(&source).unwrap();
    let store = MediaChunkStore::at(dir.path().join("chunks"));
    store.put_chunk(b"verified replacement").unwrap();
    // Use an actual leaf path without changing the process working directory.
    let target = tempfile::NamedTempFile::new_in(".")
        .unwrap()
        .into_temp_path();
    let relative = Path::new(target.file_name().unwrap());
    fs::write(relative, b"old contents").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(relative, fs::Permissions::from_mode(0o750)).unwrap();
    }
    chunk_store::reassemble(&manifest, &store, relative).unwrap();
    assert_eq!(fs::read(relative).unwrap(), b"verified replacement");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(relative).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }
    manifest.media_oid = "a".repeat(64);
    assert!(chunk_store::reassemble(&manifest, &store, relative).is_err());
    assert_eq!(fs::read(relative).unwrap(), b"verified replacement");
}

/// Connection file written by the monoengine FC-15 harness (JSON only).
#[derive(Debug)]
struct ReadyFile {
    lfs_url: url::Url,
    token: String,
}

fn parse_ready_json(bytes: &[u8]) -> Result<ReadyFile, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("ready-file is not valid JSON: {e}"))?;
    let lfs_url = value
        .get("lfs_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "ready-file is missing lfs_url".to_string())?;
    let token = value
        .get("token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "ready-file is missing token".to_string())?;
    let lfs_url =
        url::Url::parse(lfs_url).map_err(|e| format!("ready-file lfs_url is not a URL: {e}"))?;
    if !lfs_url.username().is_empty() || lfs_url.password().is_some() {
        return Err("ready-file lfs_url must not include credentials".to_owned());
    }
    if !lfs_url.path().contains("/info/lfs") {
        return Err(format!(
            "ready-file lfs_url must keep the repository LFS URL (.../<repo>.git/info/lfs/), got {lfs_url}"
        ));
    }
    Ok(ReadyFile {
        lfs_url,
        token: token.to_owned(),
    })
}

fn load_ready_file() -> ReadyFile {
    let path = std::env::var("MONOENGINE_FASTCDC_READY_FILE").unwrap_or_else(|_| {
        panic!("MONOENGINE_FASTCDC_READY_FILE is required (JSON object with lfs_url and token)")
    });
    let bytes = fs::read(&path)
        .unwrap_or_else(|e| panic!("failed to read MONOENGINE_FASTCDC_READY_FILE {path}: {e}"));
    parse_ready_json(&bytes).unwrap_or_else(|e| panic!("{e}"))
}

fn remote_from_lfs_url(lfs_url: &url::Url) -> String {
    let s = lfs_url.as_str();
    s.strip_suffix("/info/lfs/")
        .or_else(|| s.strip_suffix("/info/lfs"))
        .unwrap_or_else(|| {
            panic!("ready-file lfs_url must be a repository LFS URL ending in /info/lfs/, got {s}")
        })
        .to_owned()
}

#[test]
fn ready_file_rejects_malformed_payloads() {
    assert!(
        parse_ready_json(b"not-json")
            .unwrap_err()
            .contains("valid JSON")
    );
    assert!(
        parse_ready_json(br#"{"token":"t"}"#)
            .unwrap_err()
            .contains("missing lfs_url")
    );
    assert!(
        parse_ready_json(br#"{"lfs_url":"http://127.0.0.1:9/acme/app.git/info/lfs/"}"#)
            .unwrap_err()
            .contains("missing token")
    );
    assert!(
        parse_ready_json(br#"{"lfs_url":"http://127.0.0.1:9/","token":"t"}"#)
            .unwrap_err()
            .contains("repository LFS URL")
    );
    assert!(
        parse_ready_json(
            br#"{"lfs_url":"http://user:password@127.0.0.1:9/acme/app.git/info/lfs/","token":"once"}"#,
        )
        .unwrap_err()
        .contains("credentials")
    );
    let ok = parse_ready_json(
        br#"{"lfs_url":"http://127.0.0.1:9/acme/app.git/info/lfs/","token":"once"}"#,
    )
    .unwrap();
    assert_eq!(ok.token, "once");
    assert!(ok.lfs_url.as_str().contains("/info/lfs"));
}

// FC-15 seeds this credential for a distinct user on its disposable server.
const OTHER_USER_AUTHORIZATION: &str = "Bearer other-user-not-in-ready-file";

/// Verify authentication and both read routes directly: discovery intentionally
/// treats 401 as fallback, so it cannot prove authenticated scope isolation.
async fn verify_other_user_scope(
    client: &reqwest::Client,
    base: &url::Url,
    oid: &str,
    chunk_hash: &str,
) -> anyhow::Result<()> {
    use reqwest::StatusCode;

    for (path, expected, label) in [
        ("capabilities".to_owned(), StatusCode::OK, "authentication"),
        (
            format!("manifests/by-media/{oid}"),
            StatusCode::NOT_FOUND,
            "manifest isolation",
        ),
        (
            format!("manifests/by-media/{oid}/chunks/{chunk_hash}"),
            StatusCode::NOT_FOUND,
            "chunk isolation",
        ),
    ] {
        let status = client
            .get(base.join(&path)?)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?
            .status();
        anyhow::ensure!(
            status == expected,
            "second-user {label}: expected HTTP {expected}, got {status}; seed a distinct authenticated user in the live harness"
        );
    }
    Ok(())
}

#[tokio::test]
async fn other_user_scope_requires_authentication_and_denies_both_read_routes() {
    use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
    use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

    // Exercise the live guard against missing credentials and either leaked
    // read route, including chunk leakage behind a correctly hidden manifest.
    for (capabilities, manifest, chunk, error) in [
        (
            StatusCode::OK,
            StatusCode::NOT_FOUND,
            StatusCode::NOT_FOUND,
            None,
        ),
        (
            StatusCode::UNAUTHORIZED,
            StatusCode::NOT_FOUND,
            StatusCode::NOT_FOUND,
            Some("authentication"),
        ),
        (
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::NOT_FOUND,
            Some("authentication"),
        ),
        (
            StatusCode::OK,
            StatusCode::OK,
            StatusCode::NOT_FOUND,
            Some("manifest isolation"),
        ),
        (
            StatusCode::OK,
            StatusCode::NOT_FOUND,
            StatusCode::OK,
            Some("chunk isolation"),
        ),
        (
            StatusCode::OK,
            StatusCode::UNAUTHORIZED,
            StatusCode::NOT_FOUND,
            Some("manifest isolation"),
        ),
    ] {
        let app = Router::new()
            .route("/capabilities", get(move || async move { capabilities }))
            .route(
                "/manifests/by-media/oid",
                get(move || async move { manifest }),
            )
            .route(
                "/manifests/by-media/oid/chunks/hash",
                get(move || async move { chunk }),
            )
            .layer(axum::middleware::from_fn(
                |request: axum::extract::Request, next: axum::middleware::Next| async move {
                    if request.headers().get(AUTHORIZATION)
                        != Some(&HeaderValue::from_static(OTHER_USER_AUTHORIZATION))
                    {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    next.run(request).await
                },
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder()
            .no_proxy()
            .default_headers(HeaderMap::from_iter([(
                AUTHORIZATION,
                HeaderValue::from_static(OTHER_USER_AUTHORIZATION),
            )]))
            .build()
            .unwrap();
        let result = verify_other_user_scope(&client, &base, "oid", "hash").await;
        let anonymous = reqwest::Client::builder().no_proxy().build().unwrap();
        let anonymous_result = verify_other_user_scope(&anonymous, &base, "oid", "hash").await;
        server.abort();
        assert!(
            anonymous_result
                .unwrap_err()
                .to_string()
                .contains("authentication")
        );
        match error {
            Some(label) => assert!(result.unwrap_err().to_string().contains(label)),
            None => result.unwrap(),
        }
    }
}

/// Run against a real FastCDC Media server (monoengine) using a ready-file.
/// FC-15 writes `MONOENGINE_FASTCDC_READY_FILE` with `{lfs_url, token}` only.
#[tokio::test]
#[ignore = "requires MONOENGINE_FASTCDC_READY_FILE from the monoengine FC-15 harness"]
#[serial_test::serial(cwd)]
async fn monoengine_fastcdc_http_interop() {
    use libra::{
        internal::protocol::lfs_client::LFSClient,
        utils::{
            media::{chunk_store::MediaChunkStore, manifest::MediaManifest, transfer::MediaClient},
            test::ChangeDirGuard,
        },
    };
    use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

    let ready = load_ready_file();
    let lfs_url = ready.lfs_url;
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", ready.token)).unwrap(),
    );
    let http = reqwest::Client::builder()
        .no_proxy()
        .default_headers(headers)
        .build()
        .unwrap();
    let media = MediaClient::discover(http.clone(), &lfs_url, true)
        .await
        .unwrap()
        .expect("server must negotiate FastCDC (build with --features fastcdc)");
    let dir = tempfile::tempdir().unwrap();
    ok(&["init"], dir.path());
    let _cwd = ChangeDirGuard::new(dir.path());
    let remote = remote_from_lfs_url(&lfs_url);
    let mut lfs = LFSClient::from_remote_url(&remote).unwrap();
    lfs.client = http.clone();
    let source = dir.path().join("source.bin");
    let mut seed = 0x1234_5678_9abc_def0u64;
    let data: Vec<u8> = (0..12 * 1024 * 1024)
        .map(|_| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as u8
        })
        .collect();
    fs::write(&source, &data).unwrap();
    let (manifest, _) = MediaManifest::build_from_file(&source).unwrap();
    assert!(
        manifest
            .chunks
            .windows(2)
            .any(|c| c[0].length != c[1].length)
    );
    let base = lfs_url.join("libra/media/v1/").unwrap();
    assert!(
        base.as_str().contains("/info/lfs/libra/media/v1/"),
        "Media requests must use <repo>.git/info/lfs/libra/media/v1/, got {base}"
    );

    let anon = reqwest::Client::builder().no_proxy().build().unwrap();
    let unauth = anon
        .get(base.join("capabilities").unwrap())
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(
        unauth.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "unauthenticated Media capabilities must be rejected"
    );

    let prepared: serde_json::Value = http
        .post(base.join("manifests").unwrap())
        .json(&manifest)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = prepared["manifest_id"].as_str().unwrap();
    let chunk = &manifest.chunks[0];
    http.put(
        base.join(&format!("manifests/{id}/chunks/{}", chunk.chunk_hash))
            .unwrap(),
    )
    .body(data[..chunk.length as usize].to_vec())
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap();
    // A restart resumes from the server's persisted missing-chunk response.
    assert!(lfs.push_object(&manifest.media_oid, &source).await.unwrap());
    let published: serde_json::Value = http
        .get(
            base.join(&format!("manifests/by-media/{}", manifest.media_oid))
                .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(published["manifest_id"], manifest.id().unwrap());
    let dedup: serde_json::Value = http
        .post(base.join("manifests").unwrap())
        .json(&manifest)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(dedup["missing_chunks"].as_array().unwrap().is_empty());
    assert!(lfs.push_object(&manifest.media_oid, &source).await.unwrap());
    let cache_root = dir.path().join(".libra/media/fastcdc-v2020-32k/chunks");
    let store = MediaChunkStore::at(cache_root.clone());
    // Simulate a previously downloaded first chunk, then reconstruct the rest.
    store.put_chunk(&data[..chunk.length as usize]).unwrap();
    let output = dir.path().join("download.bin");
    fs::write(&output, b"previous contents").unwrap();
    lfs.download_object(&manifest.media_oid, manifest.media_size, &output, None)
        .await
        .unwrap();
    assert_eq!(fs::read(&output).unwrap(), data);
    assert!(
        dir.path()
            .join(".libra/media/fastcdc-v2020-32k/manifests")
            .join(&manifest.media_oid)
            .join("summary.json")
            .exists(),
        "ordinary LFS download must select FastCDC and persist its manifest"
    );
    let cached = cache_root
        .join(&chunk.chunk_hash[..2])
        .join(&chunk.chunk_hash[2..]);
    fs::write(cached, b"corrupt cache").unwrap();
    lfs.download_object(&manifest.media_oid, manifest.media_size, &output, None)
        .await
        .unwrap();
    assert_eq!(fs::read(&output).unwrap(), data);
    // FC-15 seeds this token for a distinct user; it is not an invalid-token fixture.
    let other = reqwest::Client::builder()
        .no_proxy()
        .default_headers(HeaderMap::from_iter([(
            AUTHORIZATION,
            HeaderValue::from_static(OTHER_USER_AUTHORIZATION),
        )]))
        .build()
        .unwrap();
    let mut other_lfs = LFSClient::from_remote_url(&remote).unwrap();
    other_lfs.client = other.clone();
    verify_other_user_scope(&other, &base, &manifest.media_oid, &chunk.chunk_hash)
        .await
        .unwrap();
    let other_media = MediaClient::discover(other, &lfs_url, false)
        .await
        .unwrap()
        .expect("second authenticated user must negotiate FastCDC");
    let other_store = MediaChunkStore::at(dir.path().join("other-user-chunks"));
    let other_output = dir.path().join("other-user-download.bin");
    fs::write(&other_output, b"second-user destination").unwrap();
    assert!(
        !other_media
            .download(
                &manifest.media_oid,
                manifest.media_size,
                &other_output,
                &other_store
            )
            .await
            .unwrap(),
        "other users must not read scoped Media chunks"
    );
    assert_eq!(fs::read(&other_output).unwrap(), b"second-user destination");
    other_lfs
        .download_object(
            &manifest.media_oid,
            manifest.media_size,
            &other_output,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        fs::read(&other_output).unwrap(),
        data,
        "other users retain complete standard LFS access"
    );

    // Reproduce the finalize crash window: a complete basic object is present,
    // but this user's manifest has not been published. Batch now omits upload
    // actions; the normal push path must still repair the missing manifest.
    let recover = dir.path().join("recover.bin");
    fs::write(&recover, b"complete fallback without a manifest").unwrap();
    let (recover_manifest, _) = MediaManifest::build_from_file(&recover).unwrap();
    let batch: serde_json::Value = http
        .post(lfs.batch_url.clone())
        .json(&serde_json::json!({
            "operation":"upload", "transfers":["basic"], "hash_algo":"sha256",
            "objects":[{"oid":recover_manifest.media_oid,"size":recover_manifest.media_size}]
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    http.put(
        batch["objects"][0]["actions"]["upload"]["href"]
            .as_str()
            .unwrap(),
    )
    .body(fs::read(&recover).unwrap())
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap();
    assert_eq!(
        http.get(
            base.join(&format!(
                "manifests/by-media/{}",
                recover_manifest.media_oid
            ))
            .unwrap()
        )
        .send()
        .await
        .unwrap()
        .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    assert!(
        lfs.push_object(&recover_manifest.media_oid, &recover)
            .await
            .unwrap()
    );
    http.get(
        base.join(&format!(
            "manifests/by-media/{}",
            recover_manifest.media_oid
        ))
        .unwrap(),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap();
    let empty = dir.path().join("empty.bin");
    fs::write(&empty, []).unwrap();
    let (manifest, _) = MediaManifest::build_from_file(&empty).unwrap();
    assert!(lfs.push_object(&manifest.media_oid, &empty).await.unwrap());
    assert!(
        media
            .download(&manifest.media_oid, 0, &output, &store)
            .await
            .unwrap()
    );
    assert!(fs::read(&output).unwrap().is_empty());
}

fn run(args: &[&str], cwd: &Path) -> Output {
    let home = cwd.join(".libra-test-home");
    fs::create_dir_all(home.join(".config")).unwrap();
    Command::new(media_bin())
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env(
            "LIBRA_CONFIG_GLOBAL_DB",
            home.join(".libra").join("config.db"),
        )
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .expect("run libra")
}

fn ok(args: &[&str], cwd: &Path) -> Output {
    let out = run(args, cwd);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// A fresh initialized repo with a media file large enough to split into
/// several chunks (so dedup/reassembly is meaningfully exercised).
fn repo_with_media() -> (tempfile::TempDir, String) {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    ok(&["init"], p);
    // ~5 MiB of pseudo-random-but-fixed bytes → multiple content-defined chunks.
    let mut data = Vec::with_capacity(5 * 1024 * 1024);
    let mut x: u64 = 0x0BADC0DE_DEADBEEF;
    while data.len() < 5 * 1024 * 1024 {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        data.push((x >> 32) as u8);
    }
    fs::write(p.join("big.bin"), &data).unwrap();
    (repo, "big.bin".to_string())
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("json stdout")
}

#[test]
fn chunk_store_verify_roundtrip() {
    let (repo, file) = repo_with_media();
    let p = repo.path();

    let out = ok(&["--json", "media", "chunk", &file, "--store"], p);
    let js = json(&out);
    let media_oid = js["data"]["media_oid"].as_str().unwrap().to_string();
    assert_eq!(media_oid.len(), 64, "media_oid is sha256 hex");
    assert!(
        js["data"]["chunk_count"].as_u64().unwrap() > 1,
        "multi-chunk"
    );
    assert_eq!(js["data"]["algorithm"].as_str(), Some("fastcdc-v2020-32k"));

    // Paged summary + chunk store landed under a private .libra/media sibling of objects/.
    let manifest = media_ns(p)
        .join("manifests")
        .join(&media_oid)
        .join("summary.json");
    assert!(manifest.exists(), "manifest summary persisted");
    let summary_text = fs::read_to_string(&manifest).unwrap();
    assert!(
        !summary_text.contains("\"chunks\""),
        "summary must not embed the chunk list"
    );
    assert!(
        manifest
            .parent()
            .unwrap()
            .join("pages")
            .read_dir()
            .unwrap()
            .next()
            .is_some(),
        "at least one manifest page persisted"
    );
    assert!(
        media_ns(p).join("chunks").exists(),
        "chunk store dir exists"
    );
    assert!(
        !p.join(".libra").join("media").join("chunks").exists(),
        "legacy v1 chunk cache must stay absent"
    );
    // Chunks are NOT in the Git object graph.
    assert!(
        !p.join(".libra").join("objects").join("media").exists(),
        "media must not live under objects/"
    );

    // Reassemble + verify the full media_oid.
    let vout = ok(&["--json", "media", "verify", &file], p);
    assert_eq!(json(&vout)["data"]["verified"].as_bool(), Some(true));

    // The hash/offset index is derived. Deleting it must not change verify.
    fs::remove_dir_all(media_ns(p).join("index")).unwrap();
    let vout = ok(&["--json", "media", "verify", &file], p);
    assert_eq!(json(&vout)["data"]["verified"].as_bool(), Some(true));

    // Inspect the manifest.
    let iout = ok(
        &["--json", "media", "inspect", manifest.to_str().unwrap()],
        p,
    );
    assert_eq!(
        json(&iout)["data"]["oid"].as_str(),
        Some(media_oid.as_str())
    );
    assert!(
        json(&iout)["data"].get("chunks").is_none(),
        "inspect JSON is a summary, not the chunk list"
    );
    assert_eq!(
        json(&iout)["data"]["hash_algorithm"].as_str(),
        Some("sha256")
    );
}

#[test]
fn verify_fails_cleanly_on_a_corrupt_chunk() {
    let (repo, file) = repo_with_media();
    let p = repo.path();
    ok(&["media", "chunk", &file, "--store"], p);

    // Corrupt one stored chunk by truncating it.
    let chunks_dir = media_ns(p).join("chunks");
    let mut a_chunk = None;
    for shard in fs::read_dir(&chunks_dir).unwrap() {
        let shard = shard.unwrap().path();
        if shard.is_dir()
            && let Some(entry) = fs::read_dir(&shard).unwrap().next()
        {
            a_chunk = Some(entry.unwrap().path());
            break;
        }
    }
    fs::write(a_chunk.expect("a stored chunk"), b"tampered").unwrap();

    // Verify now fails (non-zero) — the corrupt chunk is caught on read, and no
    // reassembled output is produced.
    let out = run(&["media", "verify", &file], p);
    assert_ne!(
        out.status.code(),
        Some(0),
        "verify must fail on a corrupt chunk"
    );
}

/// A local media server exercises summary prepare, pages, seal, paged missing,
/// retryable finalize, and a pinned download. A 401 after prepare fails closed.
#[tokio::test]
#[serial_test::serial(cwd)]
async fn paged_upload_finalize_and_download_round_trip() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use axum::{
        Json, Router,
        body::Bytes,
        extract::{Path, Query},
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post, put},
    };
    use libra::utils::{
        media::{
            chunk_store::MediaChunkStore,
            manifest::{ChunkEntry, ManifestSummary},
            transfer::MediaClient,
        },
        test::ChangeDirGuard,
    };
    use serde_json::Value;

    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    ok(&["init"], p);
    let _cwd = ChangeDirGuard::new(p);
    let source = p.join("clip.bin");
    let bytes: Vec<u8> = (0..50_000u32).map(|n| (n % 251) as u8).collect();
    fs::write(&source, &bytes).unwrap();
    let oid = hex::encode(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
    let size = bytes.len() as u64;
    let pages_box: Arc<Mutex<Vec<Vec<ChunkEntry>>>> = Arc::new(Mutex::new(Vec::new()));
    let puts = Arc::new(AtomicUsize::new(0));
    let polls = Arc::new(AtomicUsize::new(0));
    let published = Arc::new(AtomicBool::new(false));
    let prepared: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));

    let app = Router::new()
        .route(
            "/repo.git/info/lfs/libra/media/v1/capabilities",
            get(|| async { Json(supported_capabilities(true)) }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/manifests",
            post({
                let prepared = prepared.clone();
                move |body: Bytes| {
                    let prepared = prepared.clone();
                    async move {
                        let value: Value = serde_json::from_slice(&body).unwrap();
                        assert!(value.get("chunks").is_none(), "prepare is a summary");
                        let manifest_id = value["manifest_id"].as_str().unwrap().to_owned();
                        *prepared.lock().unwrap() = Some(value);
                        Json(serde_json::json!({
                            "manifest_id": manifest_id,
                            "missing_chunks": [],
                        }))
                    }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/manifests/{id}/pages/{page_no}",
            put({
                let pages_box = pages_box.clone();
                move |Path((_, page_no)): Path<(String, u32)>, body: Bytes| {
                    let pages_box = pages_box.clone();
                    async move {
                        let page: Value = serde_json::from_slice(&body).unwrap();
                        let entries: Vec<ChunkEntry> =
                            serde_json::from_value(page["entries"].clone()).unwrap();
                        let mut pages = pages_box.lock().unwrap();
                        if pages.len() == page_no as usize {
                            pages.push(entries);
                        }
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/manifests/{id}/seal",
            post({
                let prepared = prepared.clone();
                move || {
                    let prepared = prepared.clone();
                    async move {
                        let summary = prepared.lock().unwrap().clone().unwrap();
                        Json(serde_json::json!({
                            "manifest_id": summary["manifest_id"],
                            "sealed": true,
                            "seal_generation": 1,
                            "page_count": summary["page_count"],
                        }))
                    }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/manifests/{id}/missing",
            get({
                let pages_box = pages_box.clone();
                move |Query(query): Query<std::collections::HashMap<String, String>>| {
                    let pages_box = pages_box.clone();
                    async move {
                        let pages = pages_box.lock().unwrap();
                        let flat: Vec<ChunkEntry> = pages.iter().flatten().cloned().collect();
                        let cursor = query.get("cursor").and_then(|v| v.parse::<usize>().ok());
                        if cursor.is_none() {
                            let hash = flat[0].chunk_hash.clone();
                            return Json(serde_json::json!({
                                "hashes": [hash.clone(), hash],
                                "next_cursor": "1",
                            }));
                        }
                        let start = cursor.unwrap().min(flat.len());
                        Json(serde_json::json!({
                            "hashes": flat[start..]
                                .iter()
                                .map(|chunk| &chunk.chunk_hash)
                                .collect::<Vec<_>>(),
                            "next_cursor": Value::Null,
                        }))
                    }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/manifests/{id}/chunks/{hash}",
            put({
                let puts = puts.clone();
                move |body: Bytes| {
                    let puts = puts.clone();
                    async move {
                        puts.fetch_add(1, Ordering::SeqCst);
                        let _ = body;
                        StatusCode::CREATED
                    }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/manifests/{id}/finalize",
            post({
                let prepared = prepared.clone();
                move || {
                    let prepared = prepared.clone();
                    async move {
                        let summary = prepared.lock().unwrap().clone().unwrap();
                        (
                            StatusCode::ACCEPTED,
                            Json(serde_json::json!({
                                "task_id": "task-1",
                                "manifest_id": summary["manifest_id"],
                                "state": "pending",
                                "status_url": "libra/media/v1/tasks/task-1",
                            })),
                        )
                    }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/tasks/{task_id}",
            get({
                let polls = polls.clone();
                let published = published.clone();
                let prepared = prepared.clone();
                move || {
                    let polls = polls.clone();
                    let published = published.clone();
                    let prepared = prepared.clone();
                    async move {
                        let summary = prepared.lock().unwrap().clone().unwrap();
                        let n = polls.fetch_add(1, Ordering::SeqCst);
                        if n == 0 {
                            return Json(serde_json::json!({
                                "task_id": "task-1",
                                "manifest_id": summary["manifest_id"],
                                "state": "failed",
                                "retryable": true,
                                "error_code": "io",
                            }));
                        }
                        published.store(true, Ordering::SeqCst);
                        Json(serde_json::json!({
                            "task_id": "task-1",
                            "manifest_id": summary["manifest_id"],
                            "state": "complete",
                            "oid": summary["oid"],
                            "size": summary["size"],
                        }))
                    }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/manifests/by-media/{oid}",
            get({
                let published = published.clone();
                let prepared = prepared.clone();
                move || {
                    let published = published.clone();
                    let prepared = prepared.clone();
                    async move {
                        if !published.load(Ordering::SeqCst) {
                            return StatusCode::NOT_FOUND.into_response();
                        }
                        Json(prepared.lock().unwrap().clone().unwrap()).into_response()
                    }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/finalized/{id}",
            get({
                let prepared = prepared.clone();
                move || {
                    let prepared = prepared.clone();
                    async move { Json(prepared.lock().unwrap().clone().unwrap()) }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/finalized/{id}/pages",
            get({
                let pages_box = pages_box.clone();
                let prepared = prepared.clone();
                move |Query(query): Query<std::collections::HashMap<String, String>>| {
                    let pages_box = pages_box.clone();
                    let prepared = prepared.clone();
                    async move {
                        let summary = prepared.lock().unwrap().clone().unwrap();
                        let pages = pages_box.lock().unwrap();
                        let page_no = query
                            .get("cursor")
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0);
                        let entries = pages.get(page_no).cloned().unwrap_or_default();
                        let offset_start: u64 = pages[..page_no]
                            .iter()
                            .flatten()
                            .map(|chunk| chunk.length)
                            .sum();
                        let span: u64 = entries.iter().map(|chunk| chunk.length).sum();
                        let next = if page_no + 1 < pages.len() {
                            Value::String((page_no + 1).to_string())
                        } else {
                            Value::Null
                        };
                        Json(serde_json::json!({
                            "manifest_id": summary["manifest_id"],
                            "pages": [{
                                "page_no": page_no,
                                "offset_start": offset_start,
                                "offset_end": offset_start + span,
                                "entries": entries,
                            }],
                            "next_cursor": next,
                        }))
                    }
                }
            }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/finalized/{id}/chunks/{hash}",
            get({
                let pages_box = pages_box.clone();
                let source = source.clone();
                move |Path((_, hash)): Path<(String, String)>| {
                    let pages_box = pages_box.clone();
                    let source = source.clone();
                    async move {
                        let pages = pages_box.lock().unwrap();
                        let chunk = pages
                            .iter()
                            .flatten()
                            .find(|chunk| chunk.chunk_hash == hash)
                            .cloned()
                            .unwrap();
                        let file = fs::read(&source).unwrap();
                        let start = chunk.offset as usize;
                        let end = start + chunk.length as usize;
                        file[start..end].to_vec()
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let endpoint = format!("http://127.0.0.1:{port}/repo.git/info/lfs/");
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let media = MediaClient::discover(http, &url::Url::parse(&endpoint).unwrap(), false)
        .await
        .unwrap()
        .expect("paged server");
    assert!(media.upload(&oid, size, &source).await.unwrap());
    let unique = {
        let pages = pages_box.lock().unwrap();
        pages
            .iter()
            .flatten()
            .map(|chunk| chunk.chunk_hash.clone())
            .collect::<std::collections::HashSet<_>>()
            .len()
    };
    assert_eq!(
        puts.load(Ordering::SeqCst),
        unique,
        "duplicate missing hash is one PUT"
    );
    assert!(
        polls.load(Ordering::SeqCst) >= 2,
        "retryable finalize is re-polled"
    );

    let dest = p.join("out.bin");
    let store = MediaChunkStore::open();
    assert!(media.download(&oid, size, &dest, &store).await.unwrap());
    assert_eq!(fs::read(&dest).unwrap(), bytes);

    let summary: ManifestSummary = serde_json::from_slice(
        &fs::read(
            media_ns(p)
                .join("manifests")
                .join(&oid)
                .join("summary.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(summary.oid, oid);
    assert_eq!(summary.size, size);
    assert!(!p.join(".libra/media/chunks").exists());

    let denied = Router::new()
        .route(
            "/repo.git/info/lfs/libra/media/v1/capabilities",
            get(|| async { Json(supported_capabilities(true)) }),
        )
        .route(
            "/repo.git/info/lfs/libra/media/v1/manifests",
            post(|| async { StatusCode::UNAUTHORIZED }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, denied).await.unwrap();
    });
    let denied_url = format!("http://127.0.0.1:{port}/repo.git/info/lfs/");
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let media = MediaClient::discover(http, &url::Url::parse(&denied_url).unwrap(), true)
        .await
        .unwrap()
        .expect("capability is public");
    let err = media.upload(&oid, size, &source).await.unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("401"), "{message}");
    assert!(!message.contains("\"manifest_id\""), "{message}");
}

#[test]
fn legacy_v1_cache_survives_a_namespaced_chunk() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    ok(&["init"], p);
    let legacy_chunk = p.join(".libra/media/chunks/ab/legacy");
    let legacy_manifest = p.join(".libra/media/manifests/legacy.json");
    fs::create_dir_all(legacy_chunk.parent().unwrap()).unwrap();
    fs::create_dir_all(legacy_manifest.parent().unwrap()).unwrap();
    fs::write(&legacy_chunk, b"keep-me").unwrap();
    fs::write(&legacy_manifest, b"{\"v\":1}").unwrap();

    let src = p.join("clip.bin");
    fs::write(&src, b"namespace me").unwrap();
    let out = ok(
        &["--json", "media", "chunk", src.to_str().unwrap(), "--store"],
        p,
    );
    let js = json(&out);
    let oid = js["data"]["media_oid"].as_str().unwrap();
    assert_eq!(fs::read(&legacy_chunk).unwrap(), b"keep-me");
    assert_eq!(fs::read(&legacy_manifest).unwrap(), b"{\"v\":1}");
    assert!(
        media_ns(p)
            .join("manifests")
            .join(oid)
            .join("summary.json")
            .is_file()
    );
    assert!(!p.join(".libra/media/manifests").join(oid).exists());
}

#[test]
fn probe_unreachable_endpoint_falls_back_to_standard_lfs() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    ok(&["init"], p);
    // A refused loopback port → immediate no-endpoint, no external network.
    ok(
        &["config", "remote.origin.url", "https://127.0.0.1:1/x.git"],
        p,
    );

    let out = ok(&["--json", "media", "probe", "--remote", "origin"], p);
    let js = json(&out);
    assert_eq!(
        js["data"]["chunked"].as_bool(),
        Some(false),
        "must fall back"
    );
    assert_eq!(
        js["data"]["decision"].as_str(),
        Some("standard-lfs (fallback)")
    );
    assert_eq!(
        js["data"]["reason"].as_str(),
        Some("no-capability-endpoint")
    );
}
