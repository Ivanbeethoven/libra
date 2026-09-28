# `libra verify-pack`

Validate a Git pack index (`.idx`) against its matching pack archive (`.pack`).

## Synopsis

```bash
libra verify-pack [OPTIONS] <IDX_FILE>...
```

## Description

`libra verify-pack` is a read-only plumbing command. It parses the pack index,
decodes the corresponding pack file, and verifies that both files agree on:

- index version and structural layout
- fanout table monotonicity and object-name sorting
- index checksum
- pack checksum stored in the index trailer
- object count, object IDs, and offsets
- CRC32 values for version 2 indexes

By default the pack path is derived by replacing each index file extension with
`.pack`. Use `--pack <PACK_FILE>` with a single `<IDX_FILE>` when the pack
archive lives elsewhere.

When run inside a repository, `verify-pack` uses that repository's
`core.objectformat` (`sha1`, `sha256`, or `blake3`) and never guesses the hash
kind from index layout (sha256 and blake3 share a 32-byte OID width). Outside a
repository, `--hash-kind <sha1|sha256|blake3>` is required; omitting it fails
with `LBR-CLI-002` and a hint to run inside a repository or pass the flag.
Version 1 indexes remain SHA-1 only.

Compatibility note: multiple `<IDX_FILE>` values are verified in order. `--pack`
cannot be combined with multiple indexes because Git's pack/index naming model
does not provide an unambiguous explicit pack for each index.

## Options

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `<IDX_FILE>...` | | Pack index file(s) to verify | Required |
| `--pack <PATH>` | | Pack archive to verify against one index | `<IDX_FILE>` with `.pack` extension |
| `--hash-kind <KIND>` | | Object format used to parse the index (`sha1`, `sha256`, or `blake3`). Required outside a repository; inside a repository must match `core.objectformat` when set | Repository `core.objectformat` when inside a repo |
| `--verbose` | `-v` | Print each indexed object using Git-compatible verbose fields | Off |
| `--stat-only` | `-s` | Print only Git-style non-delta and delta-chain statistics | Off |
| `--json` | | Emit a structured JSON envelope | Off |
| `--machine` | | Emit the same envelope as one compact JSON line | Off |

## Examples

```bash
libra verify-pack objects/pack/pack-abc123.idx
libra verify-pack pack-a.idx pack-b.idx
libra verify-pack --pack /tmp/pack-abc123.pack /tmp/pack-abc123.idx
libra verify-pack --hash-kind blake3 /tmp/pack-blake3.idx
libra verify-pack -v pack-abc123.idx
libra verify-pack -s pack-abc123.idx
libra verify-pack pack-abc123.idx --json
```

## Human Output

Successful non-verbose verification prints one summary line per index:

```text
objects/pack/pack-abc123.idx: ok
objects/pack/pack-def456.idx: ok
```

Verbose mode prints indexed objects before the summary line using Git's base
field layout:

```text
3b18e512dba79e4c8300dd08aeb37f8e728b8dad blob 12 21 48
objects/pack/pack-abc123.idx: ok
```

The fields are `<oid> <type> <size> <size-in-pack> <offset>`. CRC32 values for
version 2 indexes are validated and remain available in structured output, but
are not printed in human verbose mode.

Stat-only mode prints Git-style aggregate statistics and omits the trailing
`: ok` line:

```text
non delta: 42 objects
chain length = 1: 3 objects
```

## Structured Output

```json
{
  "ok": true,
  "command": "verify-pack",
  "data": {
    "idx_file": "objects/pack/pack-abc123.idx",
    "pack_file": "objects/pack/pack-abc123.pack",
    "index_version": 2,
    "object_count": 42,
    "pack_hash": "0123456789abcdef0123456789abcdef01234567",
    "index_hash": "89abcdef0123456789abcdef0123456789abcdef",
    "verified": true
  }
}
```

When `--verbose` is combined with `--json`, `data.objects[]` contains `oid`,
`object_type`, `size`, `size_in_pack`, `offset`, and optional `crc32`.
When `--stat-only` is combined with `--json`, `data.stats` contains
`non_delta` and any `chain_lengths`.

For multiple indexes, structured output wraps per-index results:

```json
{
  "ok": true,
  "command": "verify-pack",
  "data": {
    "verified": true,
    "count": 2,
    "results": [
      {
        "idx_file": "pack-a.idx",
        "pack_file": "pack-a.pack",
        "index_version": 1,
        "object_count": 42,
        "pack_hash": "0123456789abcdef0123456789abcdef01234567",
        "index_hash": "89abcdef0123456789abcdef0123456789abcdef",
        "verified": true
      },
      {
        "idx_file": "pack-b.idx",
        "pack_file": "pack-b.pack",
        "index_version": 1,
        "object_count": 42,
        "pack_hash": "fedcba9876543210fedcba9876543210fedcba98",
        "index_hash": "76543210fedcba9876543210fedcba9876543210",
        "verified": true
      }
    ]
  }
}
```

## Compatibility

| Feature | Libra | Git | jj |
|---------|-------|-----|----|
| Verify pack index | `libra verify-pack <idx>...` | `git verify-pack <idx>...` | N/A |
| Verbose objects | `-v` / `--verbose` | `-v` | N/A |
| Stat-only mode | `-s` / `--stat-only` | `-s` / `--stat-only` | N/A |
| Explicit pack path | `--pack <path>` | N/A | N/A |
| JSON output | `--json` / `--machine` | N/A | N/A |
| Version 1 index | Supported for SHA-1 repositories | Supported | N/A |
| Version 2 index | Supported | Supported | N/A |

## Error Handling

| Scenario | StableErrorCode | Exit |
|----------|-----------------|------|
| Index file cannot be opened | `LBR-IO-001` | 128 |
| Pack file cannot be opened | `LBR-IO-001` | 128 |
| Index is malformed | `LBR-REPO-002` | 128 |
| Pack is malformed | `LBR-REPO-002` | 128 |
| Index and pack disagree | `LBR-REPO-002` | 128 |
| `--pack` used with multiple indexes | `LBR-CLI-002` | 129 |
