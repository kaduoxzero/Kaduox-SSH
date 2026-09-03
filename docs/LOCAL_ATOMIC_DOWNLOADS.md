# Local atomic downloads / 本地原子下载

## English

`TransferOptions::atomic = true` is a fail-closed create-only contract for local download destinations.

### Fresh downloads

Fresh atomic downloads use a unique sibling staging path and open it with `create_new(true)`. A pre-created file, symlink, junction/reparse point, or other object therefore cannot be silently truncated through the fresh staging path.

After the staging file is fully written, flushed, synced, and assigned the remote mtime, Kaduox-SSH publishes it with a same-directory hard-link operation. Hard-link creation is create-if-absent: if the final path already exists or races into existence, the previous destination is never removed or replaced. The staging name is removed only after the final link has been created successfully.

If the filesystem does not support the required same-filesystem hard-link operation, atomic mode fails explicitly. Use `--no-atomic` only when non-atomic replacement behavior is acceptable.

This intentionally avoids relying on platform-specific rename-overwrite behavior. In particular, Kaduox-SSH never implements an `atomic=true` local replacement as `remove(final) -> rename(staging, final)`.

### Existing final destinations

Atomic download refuses every already-existing final destination, including regular files. This is intentionally symmetric with fail-closed SFTP v3 atomic uploads: portable overwrite-atomic replacement is not advertised unless the implementation can actually guarantee it.

To overwrite an existing local file, pass `--no-atomic` explicitly.

### Resume

`--resume` intentionally uses the stable `<destination>.kaduox.part` staging name. Before reopening it, Kaduox-SSH uses lstat-style metadata and requires a regular file; symbolic links and Windows reparse points are rejected, as are partial files larger than the remote source.

The metadata-check/reopen sequence remains a TOCTOU boundary because stable Rust 1.85 does not expose one uniform cross-platform no-follow open primitive. Resume staging must therefore be used only inside local directories whose write access is trusted. Fresh atomic mode does not have this predictable-name boundary.

### Recursive downloads

Atomic recursive download performs a read-only destination preflight before creating any local directory or file. It validates existing directory components, rejects symlink/reparse/non-directory components, and requires every eventual final file path to be absent. The actual per-file publish repeats the checks, so races after the preflight still fail closed.

### Crash semantics

If the process exits after the final hard link is created but before the staging link is removed, the final file remains complete and valid; an extra staging link may remain for cleanup. The old final file is never deleted as part of atomic publication.

## 简体中文

`TransferOptions::atomic = true` 对本地下载采用 **fail-closed、只创建不覆盖** 的安全契约。

### 全新下载

Fresh atomic 下载使用唯一的同目录 staging 路径，并通过 `create_new(true)` 独占创建。攻击者预先创建的普通文件、符号链接、junction/reparse point 或其他对象都不会被 staging 打开过程静默截断。

staging 文件完整写入、flush、sync 并设置远端 mtime 后，Kaduox-SSH 使用同目录 hard-link 创建最终路径。hard-link 创建本身是 create-if-absent：最终路径已经存在或在发布时发生竞争创建时，已有目标不会被删除或覆盖。只有最终链接创建成功后才删除 staging 名称。

如果文件系统不支持所需的同文件系统 hard-link，atomic 模式会明确失败。只有能够接受非原子替换语义时才应显式使用 `--no-atomic`。

因此实现不依赖 Unix/Windows 不同的 rename-overwrite 行为，也绝不会把 `atomic=true` 实现成 `remove(final) -> rename(staging, final)`。

### 最终目标已存在

atomic 下载会拒绝所有已经存在的最终目标，包括普通文件。这与远端 SFTP v3 atomic upload 的 fail-closed 语义保持一致：无法真正保证跨平台原子覆盖时，就不宣称支持它。

需要覆盖已有本地文件时，必须显式使用 `--no-atomic`。

### 断点续传

`--resume` 为了恢复中断下载，会使用稳定的 `<destination>.kaduox.part` 名称。重新打开前使用 lstat 风格元数据检查，只接受普通文件；符号链接、Windows reparse point，以及大于远端源文件的 partial 都会被拒绝。

由于 Rust 1.85 的稳定标准库没有提供统一的跨平台 no-follow open 原语，“元数据检查 -> 重新打开”仍存在 TOCTOU 边界。因此 resume staging 只能用于本地写权限可信的目录。Fresh atomic 模式没有可预测 staging 名称这一边界。

### 递归下载

atomic 递归下载会在创建任何本地目录或文件之前先完成一次只读目标预检：验证已有目录组件、拒绝 symlink/reparse/non-directory 组件，并要求所有最终文件路径当前不存在。真正发布每个文件时仍会重复检查，因此预检之后发生的竞争也会 fail-closed。

### 崩溃语义

如果进程在最终 hard-link 已创建、但 staging link 尚未删除时退出，最终文件仍是完整有效的，只可能留下额外 staging link 等待清理。atomic 发布过程中永远不会删除旧 final。