# `send-email` 策略

## 状态

`libra send-email` 有意不可用。Libra 不实现 SMTP 投递，不读取 Git 的
`sendemail.*` 配置，不管理 SMTP 凭据，也不联系邮件服务器。调用这个
未暴露的命令会在任何仓库或网络操作之前以 `LBR-CLI-001`（退出 129）失败。

这是 P2-04 / D19 兼容策略。如果只提供一个形似传输命令的
`--dry-run` / `--validate-only` 空壳，会让用户误以为 Libra 已支持 Git 的
recipient、alias、configuration、credential、TLS 和 SMTP 语义。

## 安全工作流

Libra 负责生成 patch；专用邮件工具负责校验和投递：

1. 用 `libra format-patch` 生成 Git 可消费的邮件。
2. 在本地审阅生成文件。
3. 用 stock `git send-email --dry-run` 校验外部传输配置和解析后的收件人。
4. 只有在审阅准确收件人和 mailer 配置后，才移除 `--dry-run`。

SMTP 凭据、alias、recipient expansion、TLS、重试与投递日志都属于外部
mailer。Libra 在这条路径上不会接收这些秘密。

## Examples

生成一个 patch，用 stock Git 校验，然后显式发送：

```bash
libra format-patch -1 HEAD
git send-email --dry-run 0001-*.patch
git send-email 0001-*.patch
```

把已审阅的 series 生成到专用目录：

```bash
libra format-patch --cover-letter -o outgoing origin/main..HEAD
git send-email --dry-run outgoing/*.patch
```

如果没有安装 `git send-email`，请使用能消费 RFC 2822 message 文件的其他 mailer。
不要把自定义 wrapper 命名为 `libra send-email`；脚本应显式保留传输边界。

## 兼容性

| 表面 | Libra | Stock Git |
|------|-------|-----------|
| Patch 邮件生成 | `libra format-patch` | `git format-patch` |
| SMTP 投递 | 不支持 | `git send-email` |
| 传输 dry run | 不支持 | `git send-email --dry-run` |
| `sendemail.*` 配置 / alias / 凭据 | 不读取 | 由 `git send-email` 读取 |

支持的生成能力见 [`format-patch.md`](format-patch.md)，命令级 tier 见
[`COMPATIBILITY.md`](../../../COMPATIBILITY.md)。
