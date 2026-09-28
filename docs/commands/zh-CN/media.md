# `libra media`

FastCDC LFS 媒体分块客户端（lore.md §6），是受 `fastcdc` 功能开关控制的 Libra 扩展，只有使用 `--features fastcdc` 构建才会编译，**默认二进制中不存在**。它按内容为媒体文件分块，构建带版本的 manifest，将块存入私有本地存储，重组并验证文件，并与远端协商分块 LFS 能力；远端不支持时回退到标准 Git LFS。

`media` 是 Libra 专有扩展（`intentionally-different`）：Git 没有媒体分块概念。它不修改 Git 对象图，chunk hash 不是 Git object ID；块和 manifest 存放在与 `objects/` 同级的私有 `.libra/media/fastcdc-v2020-32k/` 中。旧配方留下的 `.libra/media/{chunks,manifests}` 不读取、不写入、不删除。`media_oid` 始终是完整文件的 SHA-256，独立于 `core.objectformat`，与标准 LFS pointer OID 一致。


## 升级 / 恢复（C-08）

新配方写入 `.libra/media/fastcdc-v2020-32k/`，不迁移、不删除旧的 `.libra/media/{chunks,manifests}`，也不触碰标准 LFS 对象。异常时停写新空间，切回匹配的旧二进制与旧空间；不可仅 revert 代码而继续对活动数据写入。

## 子命令

| 子命令 | 说明 | 示例 |
|---|---|---|
| `chunk <path> [--store] [--prior-manifest <file>]` | 对文件做 FastCDC 分块并输出 manifest；`--store` 会把 chunks + manifest 持久化到 `.libra/media/fastcdc-v2020-32k`。`--prior-manifest`（必须配合 `--store`）按 ADR-FL-04 做同长度 coherence：只复用 hash 匹配的旧块，变化区重切，长度变化则冷切；非法 prior 失败且不发布新缓存布局。 | `libra media chunk edit.psd --prior-manifest old/summary.json --store` |
| `inspect <manifest>` | 校验分页 manifest 摘要（或单页信封）并打印摘要，不展开全部 chunk。 | `libra media inspect .libra/media/fastcdc-v2020-32k/manifests/<oid>/summary.json` |
| `verify <path> \| --media-oid <oid>` | 从本地 chunk store 重组并验证完整 `media_oid`（永不发布损坏文件）。 | `libra media verify big.psd` |
| `probe [--remote <name>]` | 探测远端 media capability endpoint 并报告传输决策（chunked vs standard-LFS fallback）。 | `libra media probe --remote origin` |
| `fetch <path> --offset <u64> --length <u64> --output <file> [--remote <name>]` | 从已 finalize 的 Media 导出字节范围到**新**片段文件（ADR-FL-03）。解析 `path` 上的 LFS/Media 指针，固定 `manifest_id`，只取覆盖页，并 GET 覆盖集合减去有效缓存的唯一 hash。拒绝覆盖已存在目标或 symlink；不改 hydrate、tracked 指针或完整 LFS 缓存。零 `length` 在 `offset ≤ size` 时允许。显式范围导出禁止整对象 LFS fallback。JSON 标明 C-07 信任边界：认证覆盖块不能独立证明全文件 oid/SHA-256。与 [`hydrate`](hydrate.md) 不同，本命令写入独立切片文件，不替换 tracked 路径。 | `libra media fetch asset.bin --offset 0 --length 4096 --output slice.bin` |
| `--json` | stdout 上的结构化 JSON 信封（全局标志）。 | `libra --json media chunk big.psd` |

## 安全回退

`media probe` 只报告远端能力：`chunked (fastcdc-v2020-32k)`，或 `standard-lfs (fallback)` 并附带原因，例如没有能力端点、服务端禁用、算法不兼容、所需能力不足、协议版本不兼容或退避后的服务端错误。它假定仓库允许分块且本地存在完整 fallback，**不会读取 `lfs.fastcdc`**，在这些假定下也不会报告 `blocked`。因此，probe 输出 `chunked` 不等于当前仓库已经启用实际分块传输。

实际 LFS 传输还会检查 `lfs.fastcdc`。传输开始前，没有能力端点、旧算法或分页限额不足（`manifest_paging` 必须为 `v1`，单页 4096 条、信封 1 MiB）时继续使用标准 LFS。仅提供 chunk-only 的远端回退 basic LFS。`range_read=false` 不阻止完整分块传输或 covering-chunk 的 `media fetch`。显式范围导出禁止整对象 LFS fallback。传输开始后，认证、哈希或协议失败会直接报错，不会静默改传整个对象。以 `--features fastcdc` 构建的 Mega 实现了需要认证的扩展；其他远端继续使用标准 Git LFS。

## 与 Mega 联动传输

在 Libra 源码仓库执行 `cargo build --features fastcdc`；在 Mega 仓库按正常服务配置执行 `cargo run -p mono --features fastcdc -- service http` 构建并启动 HTTP 服务。两端默认构建均关闭该 feature。以下 `libra` 命令必须使用刚构建的二进制（`target/debug/libra`，Windows 为 `libra.exe`）；编译不会替换 PATH 中另行安装的版本。

先通过 Mega 现有的已登录用户令牌签发流程（`POST /api/v1/user/token/generate`）取得 **Mono 访问令牌**。`libra auth login` 只在本地保存已有令牌，不会替 Mega 签发令牌；GitHub PAT 或浏览器会话 cookie 不能代替 Mono access token。

以本机 8000 端口的 Mega HTTP 服务为例，在 Libra 仓库中执行：

```bash
libra config remote.origin.url http://localhost:8000/project/demo.git
libra auth login --host http://localhost:8000
# 在隐藏提示中粘贴 Mono 访问令牌。
libra auth status --host http://localhost:8000
libra config lfs.fastcdc true
libra media probe --remote origin
```

编入 feature 后，未设置 `lfs.fastcdc` 时默认允许自动协商；`true` 显式启用，`false` 在该仓库禁用传输扩展。令牌绑定的**主机和端口**必须与远端一致。非 loopback 服务必须使用 HTTPS，例如 `--host https://mega.example.com:8443`；HTTP 仅允许为 loopback 附加令牌。`--host` 只传 origin，不带仓库路径，不要将令牌放进 URL。脚本通过 `--with-token` 从 stdin 读取令牌，详见 [`libra auth`](../auth.md)。

`origin` 保留仓库 URL；LFS 客户端使用 `<repo>.git/info/lfs`，在该地址后追加 `libra/media/v1/capabilities` 探测能力，并自动将本地存储的令牌附加为 Bearer header。

正常 LFS push/upload 先 `POST manifests` 提交有界摘要，再 `PUT manifests/{id}/pages/{page_no}` 上传规范页并 seal，然后只 PUT `GET manifests/{id}/missing?cursor=` 点名的 hash。游标里的重复 hash 只上传一次；磁盘索引中不存在的 hash 报错。finalize 是持久任务：`POST manifests/{id}/finalize` 返回 HTTP 202，带 `task_id` 和同源 `status_url`。客户端轮询 `GET tasks/{task_id}` 的 `pending` / `running`，对可重试的 `failed` 用同一 `task_id` 重新排队，并只在 `complete` 的 `manifest_id`、`oid`、`size` 与本地摘要一致时接受。429 遵守 `Retry-After`（限制在 1–30 秒）并在数次后停止。单次请求超时 120 秒；连续 10 分钟没有进展则报错。持续有进展的传输没有固定总时限。

下载固定 `manifest_id`。`GET manifests/by-media/{oid}` 与 `GET finalized/{manifest_id}` 的 id、oid、size 必须一致。页来自 `GET finalized/{manifest_id}/pages`，块字节来自 `GET finalized/{manifest_id}/chunks/{hash}`。重组后的 SHA-256 匹配后才替换目标。远端清单或块损坏会报错，保留已有目标文件；没有 manifest、能力不兼容或功能被禁用时使用标准完整对象 LFS。prepare 之后的协议失败不会回退。

本地分块没有全文件字节数、chunk 条数或 manifest 大小的产品上限。`media chunk` 按页产出布局（每页最多 4096 条，紧凑 entries 数组最多 960 KiB，信封最多 1 MiB）并只打印摘要。分页边界不参与 canonical manifest id。`--store` 写入 `.libra/media/fastcdc-v2020-32k/manifests/<oid>/summary.json`、不可变 `pages/<n>.json`、chunk 字节，以及 `.libra/media/fastcdc-v2020-32k/index/local/<manifest_id>/` 下的派生 hash/offset 索引。该索引可以删除，verify 时会从页重建，它不是仓库配置。仍要求整包 manifest 的服务端只在紧凑 JSON 能放进 1 MiB 信封时重试一次；更大的布局直接失败。

`media chunk --prior-manifest <file> --store` 是明确的 prior coherence 入口（ADR-FL-04）。`<file>` 可以是分页 summary 路径/目录，也可以是整包 manifest JSON。同长度编辑会按旧块 offset 比较新字节 hash，只复用匹配块；变化区重切，若产生不足 32 KiB 的非尾片段则吸收相邻复用块后重切，必要时退回全文件冷切。长度变化（插入/删除）按冷切处理，不声称历史边界优化。损坏的 prior 报错，且不向新 oid 发布缓存布局。上传优先消费该缓存：prepare 前重新检查源 size、全文件 oid 与逐块 hash，源文件变化则失败关闭且不向远端提交；缓存缺失/被驱逐时可冷切。

Mega 当前按「认证用户＋仓库路径」隔离块和 manifest，其他用户通过既有标准 LFS 完整对象路径下载。这些端点要求 Bearer 访问令牌，不提供公开的裸 chunk-hash 查询或下载；这并不等于实现了完整仓库 ACL。chunk payload 上限为 256 KiB。

Pending 描述符在 24 小时后过期，重新准备 manifest 可继续查询和上传缺块；过期数据不会自动回收。此扩展需要显式启用，部署前应规划保留策略与配额，不能对仍被已发布 manifest 引用的块直接设置生命周期删除。

## 延后项

共享仓库 ACL、自动孤儿块 GC、配额统计、服务端 fsck/heal、obliteration、仅存块策略和按字节范围水合尚未实现。当前传输扩展不代表已完成 Lore §6.5–6.8 的全部生产要求。

## 示例

```bash
libra media chunk big.psd                 # 对文件分块；打印 manifest 摘要
libra media chunk big.psd --store         # 同时本地持久化 chunks + manifest
libra media chunk edit.psd --prior-manifest old/summary.json --store
libra media inspect .libra/media/fastcdc-v2020-32k/manifests/<oid>/summary.json
libra media verify big.psd                # 从 store 重组并验证 media_oid
libra media probe --remote origin         # capability probe；回退到标准 LFS
libra --json media chunk big.psd          # 给 agents 使用的结构化 JSON 输出
```
