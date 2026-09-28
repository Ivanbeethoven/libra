# `libra media` 开发设计

## 命令实现目标

保留默认关闭的 `fastcdc = []` 功能，复用既有确定性分块器、manifest 和本地 chunk store，
把 FastCDC 接到 Libra 的真实 LFS 上传/下载路径，并与 monoengine 的可选服务端实现联动。
不增加客户端依赖，不修改 Git 对象图或标准 LFS pointer 的 SHA-256 标识。

## 对比 Git 与兼容性

`intentionally-different`：`media chunk/inspect/verify/probe` 是 Libra 扩展。
默认构建仍使用标准 LFS；启用功能后，远端没有兼容能力或 manifest 时回退完整对象。
`lfs.fastcdc=false` 可按仓库关闭传输扩展。所有远端现在都保留仓库路径，
例如 `/project/demo.git/info/lfs`。这不是标准 Git FastCDC 互通。

## 设计方案

- 算法保持冻结：`fastcdc = "=3.2.1"`，v2020，Normalization::Level1，seed=0，
  32768/65536/262144 bytes（`fastcdc-v2020-32k`）。
- `MediaManifest` 字段保持 v1。没有全文件字节数、chunk 条数或 manifest 大小的产品上限。
  本地布局是有界 summary 加不可变页；P-01a 每页最多 4096 条且紧凑 entries 数组 ≤960 KiB，
  单页/摘要/状态信封 ≤1 MiB。页边界不参与 canonical id。缓存命名空间是
  `.libra/media/fastcdc-v2020-32k/{chunks,manifests,index}`。派生 hash/offset 索引在
  `index/local/<manifest_id>/`，可删除后从页重建，不写全局配置库。
  旧 `.libra/media/{chunks,manifests}`（`fastcdc-v1`）不读取、不写入、不删除。
  `media chunk` / `inspect` / `verify` 不构造全文件 chunk `Vec` 或整包 JSON。
- `capability` 在仓库 LFS URL 后追加 `libra/media/v1/capabilities`。
  协商要求 `manifest_paging=v1`、`supports_manifest_id_read`、`batch_exists`、
  标准 LFS fallback，以及页/信封/块限额。`range_read=false` 不阻止完整分块传输。
  传输开始前能力不足回退标准 LFS；开始后认证、哈希、协议失败直接报错。
  使用 host-scoped Bearer token、单次请求 120 秒超时、有界响应。
- `transfer::MediaClient` 上传提交 summary，按页 PUT，seal 后用磁盘索引只上传
  missing 游标点名的 hash（重复 hash 一次，未知 hash 报错），再轮询持久 finalize 任务。
  `complete` 必须匹配 `manifest_id`/`oid`/`size`。可重试失败用同一 `task_id` 重新排队；
  429 遵守 `Retry-After`（1–30 秒）。无进展 10 分钟失败，持续进展没有固定总时限。
  已发布的 MF-06 `POST manifests` 仍解析完整 `MediaManifest`。客户端先提交 summary；
  收到 HTTP 400 且紧凑整包能放进 1 MiB 时重试一次。超过信封的布局失败关闭，
  不退回 basic LFS。
- 下载固定 `manifest_id`：`by-media` 与 `finalized/{id}` 的 id/oid/size 必须一致，
  页与块都走 `finalized/{id}/...`。按 offset/length 取块，不使用等长除法。
  缓存块读取时重算 SHA-256；远端坏块或坏清单拒绝发布。
- `chunk_store::reassemble_paged` 使用既有 `StreamingAtomicFile`，独占临时文件、
  错误时自动清理、完整校验后原子覆盖目标。
- `LFSClient::upload_object/download_object` 的新调用严格在 feature gate 内。
  标准 LFS batch 保持 basic，不向普通服务端发送扩展上传请求。

## 协议与权限边界

端点位于 `<repo>.git/info/lfs/libra/media/v1`：

| 方法 | 路径 |
|---|---|
| GET | /capabilities |
| POST | /manifests |
| PUT | /manifests/{id}/pages/{page_no} |
| POST | /manifests/{id}/seal |
| GET | /manifests/{id}/missing |
| PUT | /manifests/{id}/chunks/{hash} |
| POST | /manifests/{id}/finalize |
| GET | /tasks/{task_id} |
| GET | /manifests/by-media/{oid} |
| GET | /finalized/{id} |
| GET | /finalized/{id}/pages |
| GET | /finalized/{id}/chunks/{hash} |

`POST manifests` 的规范请求体是 summary（version、algorithm、hash_algorithm、oid、size、
chunk_count、page_count、manifest_id、有界 created_by），返回 `manifest_id`。
页 PUT 提交 `ManifestPage`。seal 后 missing 按 cursor 分页，每页最多 4096 个 hash。
finalize 返回 202 `{task_id,manifest_id,state,status_url}`；`status_url` 必须是同源
`tasks/{task_id}` 或 `libra/media/v1/tasks/{task_id}`，客户端不跟随跨源地址。
manifest_id 是紧凑 JSON 数组
`[version,algorithm,hash_algorithm,media_oid,media_size,chunks]` 的 SHA-256，
不包含客户端 provenance。冻结边界保证同一内容的合法 manifest ID 一致。

monoengine 新端点要求 Mono access token，并保留 URI 改写前的仓库路径。
本版按「认证用户＋仓库」隔离存储，再由 manifest ID / media OID 限定对象范围。
不同用户/仓库不能查询或读到彼此的块；另一用户的下载回退既有完整 LFS 对象。
没有公开的裸 chunk-hash GET。服务端必须以 `--features fastcdc` 显式构建，
默认不暴露扩展端点。

## 测试

既有分块/manifest/cache 单测；`media_fastcdc_test` 的 CLI 测试；
坏块不覆盖目标、普通服务端完整 LFS 回退测试；
忽略的 `monoengine_fastcdc_http_interop` 连接真实 HTTP 路由和令牌验证，
覆盖实际变长分块上传/下载、只补缺块、缓存恢复/修复、跨用户拒绝和空文件。
两进程由 monoengine FC-15 启动 feature-on HTTP service 并写入
`MONOENGINE_FASTCDC_READY_FILE`（JSON 仅含 `lfs_url` 与一次性 `token`）。
`lfs_url` 必须是 `<repo>.git/info/lfs/`。默认 `cargo test` 的 feature-gate
guard 不依赖该文件。测试结果以本次实际运行记录为准；不把编译失败或
skipped/ignored 计为通过。这不是标准 Git FastCDC 互通。

FC-15 的隔离服务还须为与 ready-file token 不同的用户播种有效测试 token
`other-user-not-in-ready-file`；该固定值只用于一次性测试服务。测试必须验证
第二用户的 capabilities 返回 200、第一用户的 manifest 与 chunk 读取路由
均返回 404，不能把认证失败或 discovery fallback 计为跨用户隔离通过。
Media 拒绝读取后独立目标文件须保持原内容，随后标准 LFS 下载仍须成功。
非 ignored 的 scope-guard 回归用例覆盖认证失败、manifest 泄漏和 chunk
泄漏；本地路由回归不替代真实 monoengine 互通证据。

## 未完成项

本次交付传输链路，不宣称完成 Lore §6 的全部生产门禁。
共享仓库 ACL、自动孤儿块 GC、quota、服务端 fsck/heal、obliteration、
chunk-only 策略、字节范围水合、跨租户 dedup 均未开放。
Pending 描述符 24 小时到期，过期数据不会自动回收；部署方需明确保留策略，
不得对仍被 Finalized manifest 共享的块设置无条件生命周期删除。
