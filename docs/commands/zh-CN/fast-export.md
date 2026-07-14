# `libra fast-export`

把选定历史导出为 Git fast-import 流。本命令只读：不会修改对象、ref、index 或工作树。

## 用法

```text
libra fast-export [--all] [<rev>...]
```

## 说明

`fast-export` 按父先子后的顺序写出提交，并在整条流中共享 mark 表。无参数时导出 `HEAD`；分支或 tag 保留真实 ref 名，裸修订使用合成的 `refs/heads/exported-N`。

已支持的选择与保真能力：

- 在一条流中导出多个修订；
- 用 `A..B` 与 `^A` 排除历史以生成增量流（被排除的父以字面 prerequisite 对象 ID 写出）；
- `--all` 导出全部本地分支、tag 与 Libra notes 映射；
- lightweight tag 与指向 commit 的 annotated tag（包含 tag 对象）；
- 以合法 fast-import `N` 记录表示 notes；
- 对空白、控制字符、引号、反斜杠与 UTF-8 路径字节使用 Git C 风格 quoting；
- blob/commit/tag 共用 mark，末尾写 `done`。

每个提交都用 `deleteall` 加完整 `M` 文件清单编码，比 Git 的父 diff 流更大，但会重建同一棵树。fast-import 的 commit 记录无法表示 commit 签名头，因此导出时会省略它；annotated tag 的消息保持不变。

若 `--all` 中某条已存 note 的目标无法获得 stream mark（例如不在选中历史中），命令会 fail-closed；请扩大导出 ref 集，而不是生成会静默丢 note 的流。

## 选项

| 选项 | 说明 |
|---|---|
| `<rev>...` | 修订、`A..B` 范围或 `^A` 排除项；默认 `HEAD`。 |
| `--all` | 导出全部本地分支和 tag，以及目标位于该对象图内的 notes。 |

stdout 始终是原始协议流；全局 JSON/machine flag 不会包裹协议字节。

## 退出码

| 退出码 | 含义 |
|---|---|
| `0` | 完整流已写出。 |
| `128` | 仓库、修订、对象、note 闭包或输出失败。 |

## 示例

```bash
libra fast-export --all > repository.fi
libra fast-export main topic > selected.fi
libra fast-export v1..main > since-v1.fi
libra fast-export --all | libra fast-import --quiet
```

## 与 Git 对比

| 任务 | Libra | Git |
|---|---|---|
| 导出指定 refs | `libra fast-export main topic` | `git fast-export main topic` |
| 导出本地 refs | `libra fast-export --all` | `git fast-export --all` |

仍延后：对称范围 `A...B`、marks 文件（`--import-marks`/`--export-marks`）、`--anonymize`、path/blob 过滤，以及最终不指向 commit 的 annotated tag。
