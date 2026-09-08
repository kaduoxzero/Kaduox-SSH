//! 显式启用的真实 SSH 跳板测试；不修改主机库、凭据或远程文件。
use super::*;
use crate::commands::hosts::{open_store, route_to_dto};
use crate::credentials::stored_password;
use crate::models::SessionDto;
use crate::state::SessionEntry;
use kaduox_ssh_core::{
    Authentication, ConnectionConfig, ConnectionLease, HostKeyPolicy, JumpAuthFuture,
    JumpAuthProvider, JumpAuthRequest, JumpHost, RemoteUser,
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

struct SuppliedJumpPassword(String);

impl JumpAuthProvider for SuppliedJumpPassword {
    fn authentication<'a>(&'a mut self, request: JumpAuthRequest) -> JumpAuthFuture<'a> {
        Box::pin(async move {
            anyhow::ensure!(
                request.index == 0 && request.attempt == 1,
                "跳板认证失败，不重复尝试密码"
            );
            Ok(Some(Authentication::Password(self.0.clone())))
        })
    }
}

async fn register(state: &DesktopState, lease: ConnectionLease) {
    let config = lease.config();
    let details = SessionDto {
        alias: config.alias.clone(),
        address: config.host.clone(),
        port: config.port,
        user: config.username.clone(),
        auth_method: "password".into(),
        connected_at_unix: crate::util::now_unix().unwrap(),
        host_key: lease
            .server_host_key()
            .await
            .map(crate::commands::connection::host_key_to_dto),
        route: route_to_dto(config),
        warning: None,
    };
    state
        .sessions
        .write()
        .await
        .insert(config.alias.clone(), SessionEntry { lease, details });
}

async fn ssh_banner_through_forward(state: &DesktopState, alias: &str, target: &str) -> Result<()> {
    let forward = start_forward_inner(
        ForwardStartRequest::Local {
            alias: alias.into(),
            bind_address: "127.0.0.1".into(),
            bind_port: 0,
            target_host: target.into(),
            target_port: 22,
        },
        state,
    )
    .await?;
    println!("FORWARD_ROUTE {}", serde_json::to_string(&forward.route)?);
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let socket = tokio::net::TcpStream::connect(("127.0.0.1", forward.bind_port)).await?;
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        anyhow::ensure!(line.starts_with("SSH-2.0-"), "SSH 服务未返回有效 banner");
        println!(
            "FORWARD_OK localhost:{} -> {}:22 {}",
            forward.bind_port,
            target,
            line.trim()
        );
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("转发读取 SSH banner 超时")?;
    if let Some(control) = state.forwards.write().await.remove(&forward.id) {
        control.resource.close().await?;
    }
    result
}

#[tokio::test]
#[ignore = "requires explicit KADUOX_LIVE_JUMP_HOST/USER/PASSWORD and KADUOX_LIVE_TARGET_ALIAS"]
async fn real_saved_target_via_supplied_jump_and_local_forward() -> Result<()> {
    tokio::time::timeout(Duration::from_secs(150), run())
        .await
        .context("真实跳板测试总超时")?
}

async fn run() -> Result<()> {
    let host = std::env::var("KADUOX_LIVE_JUMP_HOST")?;
    let user = std::env::var("KADUOX_LIVE_JUMP_USER")?;
    let password = std::env::var("KADUOX_LIVE_JUMP_PASSWORD")?;
    let target_alias = std::env::var("KADUOX_LIVE_TARGET_ALIAS")?;
    let store = open_store()?;
    anyhow::ensure!(store.host(&target_alias).is_some(), "目标须为已保存主机");
    let mut target =
        kaduox_ssh_hosts::resolve_host(store.database(), &target_alias, None, None)?.config;
    anyhow::ensure!(
        target.host_key_policy != HostKeyPolicy::Insecure,
        "真实目标须启用主机密钥验证"
    );
    let state = DesktopState::default();
    let mut jump = ConnectionConfig::new(&host, &user);
    jump.alias = "ubantu linux（local）".into();
    // AcceptNew 只信任首次出现的密钥；已有密钥变更仍会拒绝，不关闭验证。
    let jump_lease = state
        .connect_saved(
            &jump.alias,
            jump.clone(),
            Authentication::Password(password.clone()),
        )
        .await
        .context("第一段：本机到跳板 SSH 登录失败")?;
    let output = jump_lease
        .exec(
            "id -un; hostname; printf '%s\\n' \"$SSH_CONNECTION\"",
            &RemoteUser::Current,
        )
        .await?;
    anyhow::ensure!(output.exit_status == Some(0), "跳板只读命令失败");
    println!(
        "JUMP_LOGIN_OK {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );
    register(&state, jump_lease).await;
    ssh_banner_through_forward(&state, &jump.alias, &target.host)
        .await
        .context("第二段：跳板到目标的 SSH 端口转发失败")?;

    let target_password =
        stored_password(&target).context("目标未保存密码：跳板转发可测试，但目标登录需要凭据")?;
    target.proxy_command = None;
    target.jump_hosts = vec![JumpHost {
        alias: jump.alias.clone(),
        host: host.clone(),
        port: 22,
        username: user,
        identity_files: Vec::new(),
        host_key_policy: HostKeyPolicy::AcceptNew,
        known_hosts_file: None,
    }];
    let mut provider = SuppliedJumpPassword(password);
    state
        .manager
        .connect_with_jump_auth(
            &target_alias,
            target.clone(),
            Authentication::Password(target_password),
            &mut provider,
            None,
        )
        .await
        .context("第三段：通过跳板登录最终目标失败")?;
    let lease = state
        .manager
        .get_lease(&target_alias)
        .await
        .context("目标连接不存在")?;
    let output = lease
        .exec(
            "id -un; hostname; printf '%s\\n' \"$SSH_CONNECTION\"",
            &RemoteUser::Current,
        )
        .await?;
    anyhow::ensure!(output.exit_status == Some(0), "最终目标只读命令失败");
    println!(
        "TARGET_VIA_JUMP_OK {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );
    anyhow::ensure!(
        lease.config().jump_hosts.len() == 1 && lease.config().jump_hosts[0].host == host,
        "未使用预期跳板"
    );
    register(&state, lease).await;
    ssh_banner_through_forward(&state, &target_alias, "127.0.0.1")
        .await
        .context("第四段：完整跳板链上的本地端口转发失败")?;
    state.sessions.write().await.clear();
    state.manager.remove(&target_alias).await?;
    state.manager.remove(&jump.alias).await?;
    println!("LIVE_TEST_OK 所有临时转发及连接已关闭，未修改主机库或远程配置");
    Ok(())
}
