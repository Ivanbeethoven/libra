# 安装 Libra

## 脚本安装器

macOS/Linux 推荐安装方式：

```bash
curl -fsSL https://download.libra.tools/install.sh | sh
```

安装器把 `libra` 放到 `$LIBRA_HOME/bin`（默认 `~/.libra/bin`），写入 shell
环境文件，并创建可选的相对 symlink：

```text
~/.libra/bin/lba -> libra
```

两个名称执行同一个二进制。整个 Libra home 目录移动后，相对目标仍有效。

## Alias 安全性与幂等性

- 全新安装默认创建 `lba`。
- 重复运行已安装的同一版本时，会修复缺失 alias，不下载或替换 `libra`。
- 有效的 `lba -> libra` 或 `lba -> $LIBRA_INSTALL_DIR/libra` symlink 会被接受，
  并刷新为相对形式。
- 名为 `lba` 的普通文件、目录或指向其他目标的 symlink 属于用户，绝不覆盖。
  安装器会警告并继续。
- 平台或文件系统无法创建 symlink 时，`libra` 安装仍成功；警告后使用完整
  `libra` 命令。

安装器不会创建二进制副本、hard link、shell function 或 profile alias。`lba` 只是
`libra` 旁的可选文件系统 symlink。

## 关闭 alias

单次调用使用 flag：

```bash
curl -fsSL https://download.libra.tools/install.sh | sh -s -- --no-alias
```

自动化安装可用环境变量：

```bash
curl -fsSL https://download.libra.tools/install.sh | LIBRA_NO_ALIAS=1 sh
```

opt-out 不会删除已有 alias，只阻止本次安装创建或刷新。如需删除已知属于
Libra 的 symlink，请显式执行：

```bash
test "$(readlink "$HOME/.libra/bin/lba" 2>/dev/null)" = libra &&
    rm "$HOME/.libra/bin/lba"
```

## 相关选项

| 选项 / 变量 | 效果 |
|-------------|------|
| `-v`, `--version <VERSION>` | 安装指定版本 |
| `-d`, `--dir <PATH>` | 覆盖二进制目录 |
| `LIBRA_INSTALL_DIR` | 二进制目录的环境变量形式 |
| `--no-modify-path` | 写 env 文件，但不编辑 shell rc 文件 |
| `--no-alias` | 本次不创建或刷新 `lba` |
| `LIBRA_NO_ALIAS=1` | `lba` 的环境变量 opt-out |

在 checkout 中运行 `sh install.sh --help` 可查看完整选项和环境变量列表。
