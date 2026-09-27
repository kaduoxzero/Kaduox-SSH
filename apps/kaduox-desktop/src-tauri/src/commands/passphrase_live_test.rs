//! 真实冒烟：加密私钥 + 系统凭据库口令的保存 / 自动取用 / 删除。
//!
//! 运行（linx1 或任何 keyring 已存密码的 Linux 主机）：
//!   KADUOX_LIVE_KEY_ALIAS=linx1 cargo test --manifest-path apps/kaduox-desktop/src-tauri/Cargo.toml -- --ignored passphrase_live
//!
//! 覆盖链路：临时生成加密 Ed25519 密钥 → 用已存密码部署公钥 → 口令存入系统
//! 凭据库 → 不带口令连接（core keyring 重试生效）→ 删除口令后同请求失败
//! （fail-closed）→ 撤回公钥。结束时清理远端 authorized_keys 与凭据库条目。

use anyhow::{Context, Result};
use kaduox_ssh_core::{Authentication, HostKeyPolicy, RemoteUser};
use std::time::Duration;

use crate::commands::hosts::open_store;
use crate::credentials::{delete_key_passphrase, save_key_passphrase, stored_password};
use crate::state::DesktopState;

/// 公钥注释标记：清理时按它从 authorized_keys 撤回，不碰其它行。
const MARKER: &str = "kaduox-passphrase-live-test";

#[tokio::test]
#[ignore = "requires KADUOX_LIVE_KEY_ALIAS（主机库中 keyring 已存密码的 Linux 主机）与本机 ssh-keygen"]
async fn encrypted_key_passphrase_roundtrip_via_os_credential_store() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(180), run())
        .await
        .context("密钥口令真实冒烟总超时")?
}

async fn run() -> Result<()> {
    let alias = std::env::var("KADUOX_LIVE_KEY_ALIAS")?;
    let store = open_store()?;
    let config = kaduox_ssh_hosts::resolve_host(store.database(), &alias, None, None)?.config;
    anyhow::ensure!(
        config.host_key_policy != HostKeyPolicy::Insecure,
        "真实目标须启用主机密钥验证"
    );
    let password = stored_password(&config).context("目标主机未在系统凭据库保存密码")?;

    // 1. 生成加密 Ed25519 密钥对（ssh-keygen 与 OpenSSH 服务端格式天然兼容）。
    let dir = tempfile::tempdir().context("创建临时目录失败")?;
    let key_path = dir.path().join("id_ed25519_live");
    let passphrase = format!("kaduox-live-{}-passphrase", std::process::id());
    let keygen = tokio::process::Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-a", "16", "-q", "-N", &passphrase, "-C", MARKER, "-f"])
        .arg(&key_path)
        .output()
        .await
        .context("启动 ssh-keygen 失败")?;
    anyhow::ensure!(
        keygen.status.success(),
        "ssh-keygen 失败：{}",
        String::from_utf8_lossy(&keygen.stderr)
    );
    let public_key = tokio::fs::read_to_string(dir.path().join("id_ed25519_live.pub"))
        .await
        .context("读取公钥失败")?;
    let public_key = public_key.trim();
    anyhow::ensure!(public_key.ends_with(MARKER), "公钥缺少注释标记");

    // 2. 用 keyring 已存密码登录并部署公钥（幂等追加）。
    let state = DesktopState::default();
    let lease = state
        .connect_saved(
            &alias,
            config.clone(),
            Authentication::Password((*password).clone()),
        )
        .await
        .context("密码登录失败（部署公钥阶段）")?;
    let deploy = format!(
        "mkdir -p ~/.ssh && chmod 700 ~/.ssh && touch ~/.ssh/authorized_keys \
         && chmod 600 ~/.ssh/authorized_keys \
         && (grep -qxF '{public_key}' ~/.ssh/authorized_keys || echo '{public_key}' >> ~/.ssh/authorized_keys)"
    );
    let output = lease.exec(&deploy, &RemoteUser::Current).await?;
    anyhow::ensure!(
        output.exit_status == Some(0),
        "部署公钥失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!("PUBKEY_DEPLOYED {alias}");

    // 3. 口令存入系统凭据库后，不带口令的私钥连接必须成功（core keyring 重试）。
    // 4. 删除口令后，同一路径连接必须失败（不静默回退到任何东西）。
    // 两步无论成败都先做完清理再上报。
    let outcome = async {
        save_key_passphrase(&key_path, &passphrase).context("保存密钥口令失败")?;
        let key_auth = Authentication::PrivateKey {
            path: key_path.clone(),
            passphrase: None,
        };
        let state_with_store = DesktopState::default();
        let stored_lease = state_with_store
            .connect_saved("passphrase-live-store", config.clone(), key_auth.clone())
            .await
            .context("凭据库已存口令但无口令连接失败")?;
        let echo = stored_lease
            .exec("echo passphrase-live-ok", &RemoteUser::Current)
            .await?;
        anyhow::ensure!(
            String::from_utf8_lossy(&echo.stdout).contains("passphrase-live-ok"),
            "已存口令连接成功但 exec 输出异常"
        );
        println!("STORED_PASSPHRASE_AUTH_OK 未提供口令，凭据库口令自动生效");

        anyhow::ensure!(
            delete_key_passphrase(&key_path).context("删除密钥口令失败")?,
            "密钥口令条目应存在并被删除"
        );
        let state_after_delete = DesktopState::default();
        let denied = state_after_delete
            .connect_saved("passphrase-live-denied", config.clone(), key_auth)
            .await;
        anyhow::ensure!(denied.is_err(), "删除口令后无口令连接不应成功");
        println!("PASSPHRASE_DELETE_FAILCLOSED_OK 删除口令后连接按预期失败");
        Ok::<(), anyhow::Error>(())
    }
    .await;

    // 5. 清理：撤回公钥行 + 确保凭据库条目删除（幂等）。
    let cleanup = lease
        .exec(
            &format!("sed -i '/{MARKER}/d' ~/.ssh/authorized_keys"),
            &RemoteUser::Current,
        )
        .await;
    let _ = delete_key_passphrase(&key_path);
    state.manager.remove(&alias).await?;
    match cleanup {
        Ok(output) if output.exit_status == Some(0) => {
            println!("CLEANUP_OK 公钥已撤回，凭据库条目已删除");
        }
        other => {
            eprintln!("CLEANUP_WARNING 远端公钥撤回异常：{other:?}，请手动检查 {alias} 的 authorized_keys 中 {MARKER} 行");
        }
    }

    outcome?;
    println!("LIVE_TEST_OK 密钥口令保存/自动取用/删除全链路通过");
    Ok(())
}
