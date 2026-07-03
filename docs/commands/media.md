# `libra media`

FastCDC LFS media chunking client (lore.md §6) — a **feature-gated** Libra
extension (`fastcdc`, compiled only into builds with `--features fastcdc`;
**absent from the default binary**). It content-defines chunks of a media file,
builds a versioned manifest, stores chunks in a private local store, reassembles
and verifies them, and negotiates a remote's chunked-LFS capability with a safe
fallback to standard Git LFS.

`media` is a Libra-only extension (`intentionally-different`): Git has no media
chunking concept. The Git object graph is never touched — a chunk is never a Git
object ID, and chunks/manifests live in a private `.libra/media/` store that is a
sibling of `objects/`. The `media_oid` is always SHA-256 of the full file
(independent of `core.objectformat`), byte-identical to a standard LFS pointer
OID.

## Subcommands

| Subcommand | Description | Example |
|---|---|---|
| `chunk <path> [--store]` | FastCDC-chunk a file and emit its manifest; `--store` persists chunks + manifest to `.libra/media`. | `libra media chunk big.psd --store` |
| `inspect <manifest>` | Parse and validate a manifest JSON file. | `libra media inspect .libra/media/manifests/<oid>.json` |
| `verify <path> \| --media-oid <oid>` | Reassemble from the local chunk store and verify the full `media_oid` (never publishes a corrupt file). | `libra media verify big.psd` |
| `probe [--remote <name>]` | Probe the remote's media capability endpoint and report the transfer decision (chunked vs standard-LFS fallback). | `libra media probe --remote origin` |
| `--json` | Structured JSON envelope on stdout (global flag). | `libra --json media chunk big.psd` |

## Safe fallback

`media probe` reports one of: `chunked (fastcdc-v1)` (a fully compatible
Libra-aware media server), `standard-lfs (fallback)` with a reason (no capability
endpoint, disabled by server, incompatible algorithm, disabled by repo policy,
unknown higher version, or a server error after backoff), or `blocked` (the
server keeps no standard fallback object AND no local complete object exists — a
chunk-only upload is refused rather than silently produced). Against every
reachable remote today — none of which run the (frozen) Libra media server — the
decision is a standard Git LFS fallback.

## Deferred

The Libra-aware media **server** (real cross-machine chunked upload/download,
capability + chunk + manifest-finalize endpoints, the manifest lifecycle,
GC/fsck/heal, and every anti-side-channel guarantee) is frozen in lore.md §6.5–6.8
and not part of this client v1. Chunk-only repo policy (dropping the standard LFS
fallback object) and range-based hydration are also deferred.

## Examples

```bash
libra media chunk big.psd                 # chunk a file; print the manifest summary
libra media chunk big.psd --store         # also persist chunks + manifest locally
libra media inspect .libra/media/manifests/<oid>.json
libra media verify big.psd                # reassemble from the store and verify media_oid
libra media probe --remote origin         # capability-probe; falls back to standard LFS
libra --json media chunk big.psd          # structured JSON output for agents
```
