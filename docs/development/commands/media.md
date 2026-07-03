# `libra media` 开发设计（lore.md §6 FastCDC LFS media chunking）

## 命令实现目标

FastCDC content-defined chunking 的**诚实客户端 v1**（lore.md §6 的最后一项特性）。
严格 feature-gated（`fastcdc`，**默认关闭**，`fastcdc = []` 纯 in-tree 无新依赖），
对默认二进制/CI **零影响**。它交付客户端底座 + 能力协商/安全回退，**冻结**
Libra-aware media 服务端协议（§6.5–6.8）——因此对今天任何可达远端，能力探测都回退
标准 Git LFS。

## 对比 Git 与兼容性

`intentionally-different`：Git 没有 media chunking 概念，也不理解 Libra chunk manifest。
分类前提（§6.2）：Git object graph **不变**——FastCDC chunk **绝不**成为 Git object ID；
chunk/manifest 存于私有 `.libra/media/`（`objects/` 的物理兄弟目录，从不作为 loose object
遍历）。`media_oid` **恒为 SHA-256**（全文件哈希，独立于 `core.objectformat`），与标准 LFS
pointer 的 `oid sha256:…` 逐字节一致，保证 fallback 与端到端校验。

## 设计方案

- **模块**：`src/utils/media/`（`chunker` / `manifest` / `chunk_store` / `capability` /
  `negotiate`），全部 `#[cfg(feature = "fastcdc")]`；CLI 在 `src/command/media.rs`。
- **chunker**：in-tree 确定性 gear-hash + normalized chunking，**冻结** v1 参数
  （MIN 512 KiB / AVG 2 MiB / MAX 8 MiB，固定 256 项 GEAR 表由 splitmix64 常量构建）。同字节
  → 逐字节相同的 chunk 边界。退化契约：空输入→0 chunk；小于 MIN→单个整块。
- **manifest**：serde `MediaManifest`（version/algorithm/hash_algorithm/media_oid/media_size/
  chunks[]/created_by/fallback_oid），内容寻址存 `.libra/media/manifests/<media_oid>.json`
  （`write_atomic`，**零迁移无 SQLite 表**）。frozen schema：`crc32c` 可选字段在 v1 **留空**
  （`crc32fast` 是 IEEE CRC-32 非 Castagnoli，权威 per-chunk 完整性用 `chunk_hash` sha256）。
- **chunk_store**：`MediaChunkStore` 存**原始字节**（无 Git `<type> <len>\0`/zlib），读时按
  sha256 重校验；`reassemble` **先校验 media_oid 再 rename**（verify-then-publish，绝无坏文件）。
- **capability/negotiate**：能力探测 `libra/media/v1/capabilities`（`BasicAuth::send` 挂令牌
  + `utils::backoff::retry_idempotent` §0.2 退避），纯 `negotiate()` 决策（§6.4 矩阵，全绿默认
  Chunked，任一疑点回退标准 LFS，服务端拒 fallback + 本地无 fallback → **Block** 绝不 chunk-only
  半写）。`ProbeOutcome` 区分 NoEndpoint / ServerErrorAfterBackoff / Ok。

## CLI 面

`libra media chunk <path> [--store]` / `inspect <manifest>` / `verify <path>|--media-oid <oid>` /
`probe [--remote <name>]`，全部稳定 `--json`。不改任何既有命令默认语义。

## 诚实延后（服务端冻结，lore.md §6.5–6.8）

全部 §6.5/6.6 服务端 endpoint、真实跨机 chunked 上传/下载、manifest Pending→Finalized→
Obliterated 生命周期 + finalize CAS + 孤儿 chunk GC、**每一条 §6.7 反侧信道保证**（scope 绑定
`chunks/exists`、跨仓存在性不可区分、未 finalize manifest 不可下载）、服务端 GC/fsck/heal chunk
维护（§6.8）、chunk-only 仓库策略（§6.9 默认保留标准完整 LFS fallback）、把探测接进 live LFS
`download_object`/`push_objects` 热路径、以及对纯 git 客户端的 `.gitattributes`/git-lfs filter
bridge。§6.10 前置 (1)–(5) 均已落地；(6) §6.7 拒绝矩阵集成测试随服务端一并落地。

## 测试

`--features fastcdc` 下：chunker 单测（确定性 + 退化契约）、manifest round-trip + 校验、
negotiate() §6.4 全矩阵（含 all-green→Chunked 正例）、chunk-store 原始字节 + 重校验、
capability 状态分类；集成测试 `tests/media_fastcdc_test.rs`（chunk--store/verify 往返、坏 chunk
干净失败、probe 不可达端点回退标准 LFS）。feature-gate 由 `compat_fastcdc_feature_gate_guard`
常驻钉住（默认不入 default、cfg 门控）。

## Examples

```bash
libra media chunk big.psd                 # FastCDC-chunk a file; print the manifest summary
libra media chunk big.psd --store         # also persist chunks + manifest to .libra/media
libra media inspect .libra/media/manifests/<oid>.json
libra media verify big.psd                # reassemble from the store and verify the media_oid
libra media probe --remote origin         # capability-probe; falls back to standard LFS
libra --json media chunk big.psd          # structured JSON for agents
```

## 维护要求

修改本命令前先读 [_general.md](_general.md) 与 [_compatibility.md](_compatibility.md)。改动需同步
`COMPATIBILITY.md`、本文件、`docs/commands/media.md` 与测试。严格保持 feature-gate：任何新增
依赖必须 `optional = true`，不得进入 default。
