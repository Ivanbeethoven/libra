# `libra media`

FastCDC LFS media chunking client (lore.md §6) — a **feature-gated** Libra
extension (`fastcdc`, compiled only into builds with `--features fastcdc`;
**absent from the default binary**). It content-defines chunks of a media file,
builds a versioned manifest, stores chunks in a private local store, reassembles
and verifies them, and negotiates a remote's chunked-LFS capability with a safe
fallback to standard Git LFS.

`media` is a Libra-only extension (`intentionally-different`): Git has no media
chunking concept. The Git object graph is never touched — a chunk is never a Git
object ID, and chunks/manifests live in a private
`.libra/media/fastcdc-v2020-32k/` store that is a sibling of `objects/`. A leftover
`.libra/media/{chunks,manifests}` cache from `fastcdc-v1` is not read, written,
or deleted. The `media_oid` is always SHA-256 of the full file
(independent of `core.objectformat`), byte-identical to a standard LFS pointer
OID.


## Upgrade / recovery (C-08)

New-recipe writes go under `.libra/media/fastcdc-v2020-32k/`. The legacy
`.libra/media/{chunks,manifests}` tree and standard LFS objects are retained —
not migrated and not deleted. On a bad cutover, stop writing the new namespace
and switch back to a matching older binary + legacy space; do not only revert
code against active data.

## Subcommands

| Subcommand | Description | Example |
|---|---|---|
| `chunk <path> [--store]` | FastCDC-chunk a file and emit its manifest; `--store` persists chunks + manifest under `.libra/media/fastcdc-v2020-32k`. | `libra media chunk big.psd --store` |
| `inspect <manifest>` | Validate a paged manifest summary (or one page envelope) and print the summary. Does not dump every chunk. | `libra media inspect .libra/media/fastcdc-v2020-32k/manifests/<oid>/summary.json` |
| `verify <path> \| --media-oid <oid>` | Reassemble from the local chunk store and verify the full `media_oid` (never publishes a corrupt file). | `libra media verify big.psd` |
| `probe [--remote <name>]` | Probe the remote's media capability endpoint and report the transfer decision (chunked vs standard-LFS fallback). | `libra media probe --remote origin` |
| `--json` | Structured JSON envelope on stdout (global flag). | `libra --json media chunk big.psd` |

## Safe fallback

`media probe` reports the remote's capabilities: `chunked (fastcdc-v2020-32k)` or
`standard-lfs (fallback)` with a reason such as no capability endpoint, disabled
server support, incompatible algorithm, insufficient required capabilities,
unknown protocol version, or a server error after backoff. It assumes the
repository permits chunking and a complete local fallback object is available;
it does **not** read `lfs.fastcdc` and does not report `blocked` under these
assumptions. A `chunked` probe result therefore does not prove that transfers are
enabled in this repository.

Actual LFS transfers also apply `lfs.fastcdc`. Before a media transfer starts,
a missing capability endpoint, an old algorithm, or insufficient paging limits
(`manifest_paging` must be `v1`, with page and envelope budgets of 4096 entries
and 1 MiB) stay on standard LFS. Chunk-only advertisements use basic LFS
instead. `range_read=false` does not block a full chunked transfer. After the
transfer has started, authentication, hash, and protocol failures fail closed
and do not silently upload or download the whole object. A monoengine server built with `--features fastcdc`
implements the authenticated extension; other remotes retain the standard Git LFS
fallback. This is not standard Git FastCDC interoperability.

## Live LFS transfers with monoengine

Build Libra with `cargo build --features fastcdc` and start a monoengine HTTP
service built with `--features fastcdc` (`cargo run -p monoengine --features
fastcdc -- --config config/config.toml service http`). Both builds default to
feature OFF. The `libra` commands below must use the feature-built binary
(`target/debug/libra`, or `libra.exe` on Windows); compiling does not replace a
separately installed binary on PATH.

Obtain a **Mono-issued access token** through monoengine's existing authenticated
token-creation flow (`POST /api/v1/user/token/generate`). `libra auth login`
only stores that token locally; it does not issue a token. A GitHub PAT or
browser session cookie is not a substitute for the Mono access token.

For a local monoengine HTTP server on port 8000, run in the Libra repository:

```bash
libra config remote.origin.url http://localhost:8000/project/demo.git
libra auth login --host http://localhost:8000
# Paste the Mono access token at the hidden prompt.
libra auth status --host http://localhost:8000
libra config lfs.fastcdc true
libra media probe --remote origin
```

After compiling the feature, an unset `lfs.fastcdc` permits automatic negotiation;
`true` explicitly enables it and `false` disables it in that repository. The
stored token must match the remote's **host and port**. Use HTTPS for non-loopback
servers (for example `--host https://git.example.com:8443`); HTTP token attachment
is allowed only for loopback. Pass only the origin to `--host`, without the
repository path, and do not put tokens in URLs. For scripts, feed the token on
stdin with `--with-token`; see [`libra auth`](auth.md).

Keep the repository URL in `origin`. The LFS client preserves
`<repo>.git/info/lfs`; capability discovery appends `libra/media/v1/capabilities`
to that LFS URL. The Bearer header is attached automatically from the stored token.

Normal LFS push/upload prepares a bounded summary (`POST manifests`), puts
canonical pages (`PUT manifests/{id}/pages/{page_no}`), seals the layout, then
PUTs only hashes named by `GET manifests/{id}/missing?cursor=`. Duplicate hashes
on that cursor are uploaded once; a hash outside the disk index is an error.
Finalize is a durable task: `POST manifests/{id}/finalize` returns HTTP 202 with
`task_id` and a same-origin `status_url`. The client polls `GET tasks/{task_id}`
through `pending` and `running`, re-queues a retryable `failed` task with the
same `task_id`, and accepts `complete` only when `manifest_id`, `oid`, and
`size` match the local summary. A 429 honors `Retry-After` (clamped to 1–30
seconds) and stops after a few attempts. One request times out at 120 seconds;
ten minutes without progress is an error. There is no fixed deadline for a
transfer that keeps making progress.

Downloads pin `manifest_id`. `GET manifests/by-media/{oid}` and
`GET finalized/{manifest_id}` must agree on id, oid, and size. Pages come from
`GET finalized/{manifest_id}/pages` and chunk bytes from
`GET finalized/{manifest_id}/chunks/{hash}`. The destination is replaced only
after the reassembled SHA-256 matches. Invalid manifests or corrupted remote
chunks are errors and preserve the existing destination. No manifest, unsupported
capabilities, or a disabled feature means standard full-object LFS. A protocol
failure after prepare does not fall back.

There is no whole-file byte, chunk-count, or manifest-size product cap. `media
chunk` pages the layout (at most 4096 entries per page, compact entries array at
most 960 KiB, envelope at most 1 MiB) and prints a summary. Page boundaries are
not part of the canonical manifest id. `--store` writes
`.libra/media/fastcdc-v2020-32k/manifests/<oid>/summary.json`, immutable
`pages/<n>.json`, chunk bytes, and a derived hash/offset index under
`.libra/media/fastcdc-v2020-32k/index/local/<manifest_id>/`. That index can be
deleted and is rebuilt on verify; it is not repository config. A server that
still requires a full manifest body is retried once, and only when the compact
JSON fits in the 1 MiB envelope. A larger layout fails closed. Chunk-only
uploads are not supported. Outside a Libra repository, the public LFS download
client uses basic LFS instead of creating a repository cache.

The initial extension isolates chunks by authenticated user and repository;
another user's data is fetched through the standard full-object fallback. It
requires Bearer access tokens and does not introduce a public chunk-hash API.
Chunk payload is at most 256 KiB. This is an
opt-in transport; deployments need explicit retention and quota planning.

The ignored live test `monoengine_fastcdc_http_interop` is not Mega-only. Run it
only with `--features fastcdc` and a ready-file:

```bash
export MONOENGINE_FASTCDC_READY_FILE=/path/to/ready.json
# ready.json: {"lfs_url":"http://127.0.0.1:9000/acme/app.git/info/lfs/","token":"<one-time>"}
cargo test --features fastcdc --test media_fastcdc_test -- --ignored --exact monoengine_fastcdc_http_interop
```

`lfs_url` must stay a repository LFS URL (`<repo>.git/info/lfs/`). Do not commit
tokens. The default `cargo test` feature-gate guard still passes without this
file; the live test is executed by the monoengine FC-15 harness.

The isolated FC-15 server must also seed `other-user-not-in-ready-file` as a
valid token for a **different user** from the ready-file token. This fixed token
is a test fixture and must only be used on the disposable test server. The test
requires HTTP 200 for that user's capabilities and HTTP 404 for the first
user's manifest and chunk read routes; authentication failure cannot count as
scope isolation. It also checks that a denied Media download preserves a fresh
destination and that the second user can still download the standard LFS object.
The non-ignored scope-guard regression test covers authentication failures and
leaked manifest/chunk responses without a live server.

## Deferred

Shared repository ACLs, automatic orphan GC, quota accounting, server fsck/heal,
obliteration, chunk-only policy and byte-range hydration remain deferred. The
current implementation does not claim completion of all Lore §6.5–6.8 guarantees.

## Examples

```bash
libra media chunk big.psd                 # chunk a file; print the manifest summary
libra media chunk big.psd --store         # also persist chunks + manifest locally
libra media inspect .libra/media/fastcdc-v2020-32k/manifests/<oid>/summary.json
libra media verify big.psd                # reassemble from the store and verify media_oid
libra media probe --remote origin         # capability-probe; falls back to standard LFS
libra --json media chunk big.psd          # structured JSON output for agents
```
