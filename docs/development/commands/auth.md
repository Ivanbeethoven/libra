# auth 命令开发设计

## 命令实现目标

lore.md §1.6：token-only auth v1，令牌**生命周期同 PR 闭环**——写入（加密
落盘 0600）、读取（HTTPS 请求侧挂接）、过期检测、撤销（logout/clear）。

## 对比 Git 与兼容性

- 兼容级别：`intentionally-different`。Git 走 credential helper 外部程序
  （credential-store 明文落盘）；Libra 原生加密存储。repo 作用域
  `libra credential` helper 协议保持不变（fill 增加全局令牌静默回退）。

## 设计方案

- **存储**：属主模块 `internal::auth`（单一门面，1.5 惯例）。全局 vault
  unseal key（`~/.libra/vault-unseal-key`，0600）AES-256-GCM 加密整条记录
  `{version,host,port,username,token,expires_at,created_at}`，hex 密文存
  全局 config_kv（键 `auth.token.<sha256(host,port)>`——落盘不泄露主机名；
  credential.rs 先例 secret=false）。**OS keyring 诚实延后 2.7**（行文自身
  把 vault encrypt_token 指定为「文件 fallback 加密」；2.7 行明确「token
  auth v1 后扩展」）；0600 字面合规：写入时对全局 config.db 与 unseal key
  做 chmod 修复（Windows 依赖 profile ACL，service-token 先例）。
- **修复的既有 P1**：`lazy_init_vault_for_scope("global")` 根本不 lazy——
  每次调用都重新生成并覆盖 unseal key，旋转掉旧密钥使**所有**既有全局密文
  （加密 config 值）永久不可解。现改为先读后建。auth e2e 首跑即暴露
  （login 与 status 两个进程间密钥变了）。
- **命名空间封锁**：`auth.token.` 进 `is_sensitive_key` + `is_vault_internal_key`
  ——config get/list 不可见、set 不可伪造、unset 不可旁路注销。
- **读取侧（信任边界，STORED 令牌——交互式 401 提示仍是进程级回退且优先）**：
  `BasicAuth::send` 经 `build_split` 知晓请求 URL；仅当（1）无既有
  Authorization 头（2）scope 命中（host:port 归一化，https-only + loopback
  http 豁免——无端口存储归一 443，非 443 loopback 需显式端口登录，文档化）
  才挂 `Basic base64(user:token)`（header 标记 sensitive）；过期/不可解密
  → 警告 + auth login 提示，不挂。builder 错误按原 send 语义传播。
- **降级重定向（审阅 must-fix）**：reqwest 仅在 host/port 变化时剥
  Authorization，**scheme 不比较**——同主机 https→http 降级重定向会把令牌
  明文重发。`no_downgrade_redirect_policy()`（自定义策略：https→http 即
  error，其余 10 跳上限）应用于 https build_client 与两处 LFS builder。
- **host 归一化（审阅 must-fix）**：裸 host/host:port 先补 `https://` 再
  Url::parse（否则 `host:8443` 把 host 当 scheme）；要求 https（或 loopback
  http）、无 userinfo/path/query/fragment；小写化 + punycode + 默认端口。
- **credential fill 回退**：repo 作用域未命中且 protocol=https 时查全局
  令牌（**用户名钉定同样生效**——审阅遗漏项）；一切未命中静默 exit 0；
  store/erase 不碰 auth 令牌（gh 惯例）。
- **过期**：`--expires-at` RFC3339（裸日期拒绝并提示）/`--expires-in` 单
  单位 Nd/h/m/s（checked_mul/add 防溢出，组合形式明确拒绝）；已过期拒存；
  status 报 valid/expired/undecryptable；使用时过期 → 警告 + 提示。
- **保密不变量**：令牌绝不出现在 argv（无 --token flag）、日志、错误、
  JSON（status 输出无 token 字段——测试以密文值全文搜索钉住）。

## 还未实现的功能

| 类别 | 未完成项 | 当前处理 |
|---|---|---|
| OS keyring | 系统钥匙串后端 | 2.7；本模块边界即替换点。 |
| 交互式 OAuth/设备流 | — | 2.7。 |
| 401 自动引导 | 401 时提示 auth login 而非交互 Basic | 交互回退保持既有行为；后续项。 |

## 实现历史

- 2026-07-03（lore.md Phase 1 / 1.6）：初版全套 + 3 e2e + vault lazy-init 修复。

## 维护要求

- 改进前先读 [_general.md](_general.md)。`auth.token.*` 读写必须经
  `internal::auth`；任何新 HTTP 客户端 builder 必须应用
  `no_downgrade_redirect_policy`；令牌值绝不允许进入任何输出路径。
