# Host Inventory / 主机资产清单

## English

v0.8 introduces a small, credential-free inventory format for grouping logical SSH targets without creating a second connection-configuration system.

Connection details still come from the existing OpenSSH resolver and normal Kaduox-SSH overrides. The inventory only names targets and groups.

Default paths:

- Unix: `$XDG_CONFIG_HOME/kaduox-ssh/inventory`, otherwise `$HOME/.config/kaduox-ssh/inventory`;
- Windows: `%APPDATA%\Kaduox-SSH\inventory`;
- all platforms: `KADUOX_SSH_INVENTORY` overrides the path explicitly.

Format:

```text
# Every host used by a group is declared explicitly.
host web-01
host web-02
host db-01
host deploy@jobs-01

group web web-01 web-02
group data db-01
group production @web @data deploy@jobs-01
```

Rules:

- `host <target>` accepts the canonical Kaduox `host`, `user@host`, bracketed IPv6, or bare IPv6 target grammar;
- `group <name> <member>...` accepts declared hosts and nested groups written as `@name`;
- group names are limited to ASCII letters, digits, `.`, `_`, and `-`;
- duplicate hosts, duplicate groups, duplicate direct members, unknown references, and group cycles fail closed;
- nested expansion preserves first-seen order and removes duplicates deterministically;
- an inventory is limited to 1 MiB, 4096 hosts, 512 groups, 8192 direct group members, and 1024 expanded hosts per group.

The inventory deliberately stores no passwords, private-key passphrases, ProxyCommand secrets, or other credentials. OpenSSH aliases can therefore be grouped without copying sensitive connection material into another file.

## 简体中文

v0.8 新增一个轻量、**不保存凭据**的主机 Inventory，用于把逻辑 SSH target 组织成可复用主机组，同时避免建立第二套连接配置系统。

真实连接参数仍由现有 OpenSSH config resolver 与 Kaduox-SSH CLI override 决定；Inventory 只保存 target 与 group。

默认位置：

- Unix：优先 `$XDG_CONFIG_HOME/kaduox-ssh/inventory`，否则 `$HOME/.config/kaduox-ssh/inventory`；
- Windows：`%APPDATA%\Kaduox-SSH\inventory`；
- 所有平台都可通过 `KADUOX_SSH_INVENTORY` 显式覆盖。

示例：

```text
host web-01
host web-02
host db-01
host deploy@jobs-01

group web web-01 web-02
group data db-01
group production @web @data deploy@jobs-01
```

约束：

- `host <target>` 使用项目已有的 `host`、`user@host`、IPv6 target grammar；
- `group` 的普通成员必须先通过 `host` 声明，嵌套组使用 `@group`；
- group 名只允许 ASCII 字母、数字、`.`、`_`、`-`；
- 重复声明、重复直接成员、未知引用、循环引用都会 fail-closed；
- 嵌套展开按首次出现顺序稳定去重；
- 单文件最大 1 MiB、最多 4096 hosts、512 groups、8192 个直接 group member，单 group 展开最多 1024 hosts。

Inventory 明确不保存 password、私钥 passphrase、ProxyCommand secret 等认证数据，因此可以安全地对 OpenSSH alias 做资产分组，而不会把敏感连接材料复制到另一套配置中。
