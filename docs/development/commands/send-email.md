# `send-email` 开发策略（P2-04）

## 结论

P2-04 选择维持 `unsupported`，不实现 SMTP，也不暴露仅支持
`--dry-run` / `--validate-only` 的公开命令。No `Commands::SendEmail` variant 或
`src/command/send_email.rs` 模块存在；因此该路径无法读取传输凭据或发起网络请求。

用户可见契约是：

- Libra 用 `format-patch` 生成 RFC 2822 / mbox patch messages；
- stock `git send-email` 或另一个专用 mailer 负责 alias、recipient、credential、TLS、
  dry-run 和实际投递；
- `libra send-email` 以通用的未知子命令 `LBR-CLI-001`（退出 129）失败。

## 为什么不提供空壳 dry-run

Git `send-email --dry-run` 不只是“不打开 socket”；它仍然要解析收件人、alias、
`sendemail.*` 配置、message headers 和传输选项。一个不具备这些语义的 Libra
空壳会建立错误兼容预期，也会在未完成凭据、TLS、日志脱敏和 timeout 威胁
模型前扩大安全面。

P2-03 的 Git↔Libra `am` round-trip 已给出更小、可验收的边界：Libra 产出邮件文件，
外部 mailer 消费它们。

## 变更位置

- `COMPATIBILITY.md` 的“Git commands intentionally absent”矩阵登记 `unsupported`。
- [`_compatibility.md`](_compatibility.md) 以 D19 记录拒绝理由和重启条件。
- `docs/commands/send-email.md` 与中文文档说明安全交接流程。
- `compat_matrix_alignment::send_email_policy_is_explicit_and_non_sending` 钉死 CLI 不暴露、
  文档一致和无 SMTP 边界。

## 重启条件

只有在新 RFC 同时定义以下内容时才能公开 `send-email`：

1. Git `sendemail.*`、alias 和 recipient resolution 兼容范围；
2. SMTP/TLS 威胁模型、凭据存储、输出/日志脱敏；
3. 有界 timeout、retry/idempotency 与部分发送失败语义；
4. 可控 SMTP 服务器的 dry-run/真实投递 E2E，以及无凭据、TLS 失败和敏感数据泄漏回归。

## 验收命令

```bash
source .env.test && cargo test --test compat_matrix_alignment send_email_policy_is_explicit_and_non_sending
```
