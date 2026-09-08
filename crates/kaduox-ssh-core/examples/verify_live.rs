//! Live-server verification for connection persistence and remote metrics.
//!
//! Manual harness that exercises the real SSH stack against a reachable
//! server: password/keyboard-interactive/private-key authentication, jump
//! chains, pooled transport reuse, dead-connection eviction, the remote
//! metrics command, and SFTP upload/download round-trips.
//!
//! Requires explicit credentials (never hardcoded):
//! ```powershell
//! $env:KADUOX_LIVE_HOST='192.0.2.10'; $env:KADUOX_LIVE_USER='demo';
//! $env:KADUOX_LIVE_PASSWORD='...'; cargo run -p kaduox-ssh-core --example verify_live
//! ```

use std::time::Duration;

use kaduox_ssh_core::{
    Authentication, ConnectionConfig, ConnectionManager, HostKeyPolicy, JumpHost, RemoteUser,
    SshClient, TransferOptions,
};

const METRICS_COMMAND: &str = "printf '__KADUOX_SYSTEM_METRICS_V1__\\n'; printf 'hostname='; hostname 2>/dev/null; printf '\\nplatform='; uname -srmo 2>/dev/null; printf '\\nusername='; id -un 2>/dev/null; printf '\\nuptime='; uptime -p 2>/dev/null; printf '\\naddresses='; hostname -I 2>/dev/null; printf '\\ncpu_model='; awk -F: '/model name|Hardware|Processor/{gsub(/^[ \\t]+/,\"\",$2); print $2; exit}' /proc/cpuinfo 2>/dev/null; printf '\\ncpu_cores='; nproc 2>/dev/null; printf '\\nload='; cut -d' ' -f1-3 /proc/loadavg 2>/dev/null; printf '\\nmemory='; free -b 2>/dev/null | awk '/^Mem:/{print $2,$3,$7; exit}'; printf '\\ndisk='; df -P -B1 / 2>/dev/null | awk 'NR==2{gsub(/%/,\"\",$5); print $2,$3,$4,$5; exit}'";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let host = std::env::var("KADUOX_LIVE_HOST")
        .map_err(|_| anyhow::anyhow!("set KADUOX_LIVE_HOST to the test server address"))?;
    let user = std::env::var("KADUOX_LIVE_USER")
        .map_err(|_| anyhow::anyhow!("set KADUOX_LIVE_USER to the SSH user"))?;
    let password = std::env::var("KADUOX_LIVE_PASSWORD")
        .map_err(|_| anyhow::anyhow!("set KADUOX_LIVE_PASSWORD to the SSH password"))?;

    let mut config = ConnectionConfig::new(host.clone(), user.clone());
    config.host_key_policy = HostKeyPolicy::Insecure; // throwaway test VM
    config.keepalive_interval = Some(Duration::from_secs(10));

    let manager = ConnectionManager::default();
    let name = "live-verify";

    // [1] Password connect
    let client = manager
        .connect(
            name,
            config.clone(),
            Authentication::Password(password.clone()),
        )
        .await?;
    println!("[1] password connect ok, is_alive={}", client.is_alive());

    // [2] Two execs on the same transport (persistence check)
    let first = client.exec("echo ping-1", &RemoteUser::Current).await?;
    print!(
        "[2] exec#1 stdout={:?} status={:?}",
        String::from_utf8_lossy(&first.stdout),
        first.exit_status
    );
    let second = client.exec("echo ping-2", &RemoteUser::Current).await?;
    print!(
        "[2] exec#2 stdout={:?} status={:?}",
        String::from_utf8_lossy(&second.stdout),
        second.exit_status
    );
    println!(
        "[2] pooled connections={} (must stay 1: transport reused)",
        manager.len().await
    );

    // [3] Lease path used by the desktop app
    let lease = manager.get_lease(name).await.expect("lease must exist");
    let third = lease.exec("echo ping-3", &RemoteUser::Current).await?;
    print!(
        "[3] lease exec stdout={:?} status={:?}",
        String::from_utf8_lossy(&third.stdout),
        third.exit_status
    );

    // [4] Full remote metrics command the desktop data screen sends
    let metrics = lease.exec(METRICS_COMMAND, &RemoteUser::Current).await?;
    let raw = String::from_utf8_lossy(&metrics.stdout);
    println!("[4] metrics command exit={:?}", metrics.exit_status);
    for line in raw.lines() {
        println!("    {line}");
    }

    // [5] Dead-transport detection: close the session behind the manager's back
    client.close().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!(
        "[5] after close: is_alive={} (must be false)",
        client.is_alive()
    );

    // [6] get_lease must observe the dead entry, evict it, and return None
    let evicted = manager.get_lease(name).await.is_none();
    println!(
        "[6] get_lease on dead entry -> evicted={evicted}, pool={}",
        manager.len().await
    );

    // [7] A fresh connect for the same name must succeed (reconnect path).
    // Keyboard-interactive is disabled on this server, so this also exercises
    // the sibling-method fallback to password.
    let client = manager
        .connect(name, config, Authentication::KeyboardInteractive(password))
        .await?;
    let out = client.exec("echo revived", &RemoteUser::Current).await?;
    print!(
        "[7] reconnect exec stdout={:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    println!(
        "[7] revived is_alive={}, pool={}",
        client.is_alive(),
        manager.len().await
    );

    // [8] Private-key auth: install a fresh ed25519 key over the live session,
    // then reconnect using it.
    let key_dir = std::env::temp_dir().join("kaduox-verify-key");
    let key_path = key_dir.join("id_ed25519");
    let _ = std::fs::remove_dir_all(&key_dir);
    std::fs::create_dir_all(&key_dir)?;
    let ssh_keygen = std::process::Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            key_path.to_str().unwrap(),
            "-C",
            "kaduox-verify",
            "-q",
        ])
        .output()?;
    if !ssh_keygen.status.success() {
        anyhow::bail!(
            "ssh-keygen failed: {}",
            String::from_utf8_lossy(&ssh_keygen.stderr)
        );
    }
    let public_key = std::fs::read_to_string(key_path.with_extension("pub"))?
        .trim()
        .to_owned();
    let install = client
        .exec(
            &format!(
                "mkdir -p ~/.ssh && chmod 700 ~/.ssh && grep -qxF '{public_key}' ~/.ssh/authorized_keys 2>/dev/null || echo '{public_key}' >> ~/.ssh/authorized_keys; chmod 600 ~/.ssh/authorized_keys"
            ),
            &RemoteUser::Current,
        )
        .await?;
    println!(
        "[8] key install exit={:?} stderr={:?}",
        install.exit_status,
        String::from_utf8_lossy(&install.stderr)
    );

    let mut key_config = ConnectionConfig::new(host.clone(), user.clone());
    key_config.host_key_policy = HostKeyPolicy::Insecure;
    let key_client = SshClient::connect(
        key_config,
        Authentication::PrivateKey {
            path: key_path.clone(),
            passphrase: None,
        },
    )
    .await;
    match key_client {
        Ok(key_client) => {
            let out = key_client
                .exec("echo key-auth-ok", &RemoteUser::Current)
                .await?;
            println!(
                "[8] private-key auth ok, exec stdout={:?}",
                String::from_utf8_lossy(&out.stdout)
            );
            key_client.close().await?;
        }
        Err(error) => println!("[8] private-key auth FAILED: {error:#}"),
    }

    // [9] Jump chain: route through the server itself (jump hop then target).
    let mut jump_config = ConnectionConfig::new(host.clone(), user.clone());
    jump_config.host_key_policy = HostKeyPolicy::Insecure;
    jump_config.jump_hosts = vec![JumpHost {
        alias: "verify-jump".into(),
        host: host.clone(),
        port: 22,
        username: user.clone(),
        identity_files: vec![key_path.clone()],
        host_key_policy: HostKeyPolicy::Insecure,
        known_hosts_file: None,
    }];
    match SshClient::connect(
        jump_config,
        Authentication::PrivateKey {
            path: key_path.clone(),
            passphrase: None,
        },
    )
    .await
    {
        Ok(jump_client) => {
            let out = jump_client
                .exec("echo jump-ok", &RemoteUser::Current)
                .await?;
            println!(
                "[9] jump-chain connect ok, exec stdout={:?} alive={}",
                String::from_utf8_lossy(&out.stdout),
                jump_client.is_alive()
            );
            jump_client.close().await?;
        }
        Err(error) => println!("[9] jump-chain FAILED: {error:#}"),
    }

    // [10] SFTP: list home, upload a file (fresh + overwrite), read it back.
    {
        let entries = client.list_remote_directory(".").await?;
        println!(
            "[10] sftp list home: {} entries: {:?}",
            entries.len(),
            entries
                .iter()
                .map(|entry| entry.name.clone())
                .take(6)
                .collect::<Vec<_>>()
        );
        // Idempotent cleanup from previous runs, then verify both the fresh
        // atomic path and the overwrite path the desktop now selects.
        client
            .exec("rm -f kaduox-sftp-probe.txt", &RemoteUser::Current)
            .await?;
        let probe = key_dir.join("sftp-probe.txt");
        std::fs::write(&probe, b"kaduox sftp verify")?;

        let fresh = TransferOptions {
            atomic: true,
            ..TransferOptions::default()
        };
        client
            .upload_with_options(&probe, "kaduox-sftp-probe.txt", fresh)
            .await?;
        let stat = client.stat_remote_path("kaduox-sftp-probe.txt").await?;
        println!(
            "[10] sftp fresh atomic upload ok, size={:?} bytes",
            stat.metadata.size
        );

        let overwrite = TransferOptions {
            atomic: false,
            ..TransferOptions::default()
        };
        client
            .upload_with_options(&probe, "kaduox-sftp-probe.txt", overwrite)
            .await?;
        println!("[10] sftp overwrite upload ok");

        let download_target = key_dir.join("sftp-download.txt");
        let _ = std::fs::remove_file(&download_target);
        let download_fresh = TransferOptions {
            atomic: true,
            ..TransferOptions::default()
        };
        client
            .download_with_options("kaduox-sftp-probe.txt", &download_target, download_fresh)
            .await?;
        let content = std::fs::read_to_string(&download_target)?;
        println!("[10] sftp roundtrip content={content:?}");

        let download_overwrite = TransferOptions {
            atomic: false,
            ..TransferOptions::default()
        };
        client
            .download_with_options(
                "kaduox-sftp-probe.txt",
                &download_target,
                download_overwrite,
            )
            .await?;
        println!("[10] sftp overwrite download ok");
        client
            .exec("rm -f kaduox-sftp-probe.txt", &RemoteUser::Current)
            .await?;
    }

    // [cleanup] Remove the temporary verify key from the server.
    client
        .exec(
            "sed -i '/kaduox-verify$/d' ~/.ssh/authorized_keys 2>/dev/null || true",
            &RemoteUser::Current,
        )
        .await?;

    println!("[OK] live verification complete");
    Ok(())
}
