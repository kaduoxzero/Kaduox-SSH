//! OpenSSH 生态解析：`~/.ssh/config`（含 Include/Match）、主机目录、
//! 主机清单（inventory）与连接目标解析。纯解析，不建立 SSH 会话。

mod host_catalog;
mod inventory;
mod openssh_config_trust;
mod openssh_include;
mod openssh_match;
mod target;

pub use host_catalog::{OpenSshHostCatalog, discover_openssh_hosts};
pub use inventory::{
    HostInventory, InventoryGroup, InventoryMember, default_inventory_path, discover_inventory,
    load_inventory,
};
#[doc(hidden)]
pub use openssh_include::expand_user_config;
pub use target::ConnectionTarget;
