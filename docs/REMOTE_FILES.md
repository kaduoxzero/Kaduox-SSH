# Read-only remote file inspection

Kaduox-SSH exposes read-only remote filesystem inspection through the SFTP subsystem. These commands do not execute remote `ls` or `stat` shell commands.

## List a directory

```bash
kssh server.example.com ls
kssh server.example.com ls /var/log
kssh server.example.com ls /var/log --long
```

The default path is `.`. Directory entries are returned in stable name order.

`--long` prints the SFTP-reported file type, permission bits, owner/group when available, size, modification time, and name. Missing metadata is rendered as `-` rather than guessed.

## Inspect one path

```bash
kssh server.example.com stat /etc/ssh/sshd_config
kssh server.example.com stat /var/log/current
```

`stat` deliberately uses lstat-style semantics through SFTP `symlink_metadata`: a symbolic link is reported as a symbolic link rather than followed. When the server supports it, Kaduox-SSH also reads and prints the link target.

The public core API exposes the same data through:

- `SshClient::list_remote_directory`
- `SshClient::stat_remote_path`
- `RemoteDirEntry`
- `RemoteFileStat`
- `RemoteFileMetadata`
- `RemoteFileType`

These public types are owned by Kaduox-SSH rather than exposing `russh-sftp` protocol structures directly, keeping frontend/TUI/GUI integrations insulated from dependency internals.

## Scope and safety

This stage is intentionally read-only. It does not add remote delete, rename, chmod, chown, mkdir, or symlink creation operations. Those mutation APIs should only be introduced after the repository's transfer/sync symlink and path-boundary hardening work is integrated and validated.

The commands still establish a normal authenticated SSH connection and SFTP subsystem. Explicit connection options supplied by the caller remain in effect.

---

# 只读远端文件查看

Kaduox-SSH 现在可以通过 SFTP 子系统查看远端目录和文件元数据，不会在远端执行 `ls` 或 `stat` shell 命令。

## 查看目录

```bash
kssh server.example.com ls
kssh server.example.com ls /var/log
kssh server.example.com ls /var/log --long
```

默认路径是 `.`，目录项会按照名称稳定排序。

`--long` 会显示 SFTP 返回的文件类型、权限位、可用时的用户/组、大小、修改时间以及名称。服务器没有返回的字段显示为 `-`，不会进行猜测。

## 查看单个路径

```bash
kssh server.example.com stat /etc/ssh/sshd_config
kssh server.example.com stat /var/log/current
```

`stat` 刻意采用类似 `lstat` 的语义：通过 SFTP `symlink_metadata` 获取元数据，不会自动跟随符号链接。如果目标是 symlink，还会尝试读取并显示链接目标。

core 同时公开 `SshClient::list_remote_directory`、`SshClient::stat_remote_path` 以及稳定的 `RemoteDirEntry` / `RemoteFileStat` / `RemoteFileMetadata` / `RemoteFileType` 数据模型，后续 TUI/GUI 无需直接依赖 `russh-sftp` 的内部协议类型。

这一阶段严格保持只读，不新增远端删除、重命名、chmod、chown、mkdir 或 symlink 创建能力。破坏性文件操作应等现有 transfer/sync 的 symlink 与路径边界加固完成并通过验证后再引入。
