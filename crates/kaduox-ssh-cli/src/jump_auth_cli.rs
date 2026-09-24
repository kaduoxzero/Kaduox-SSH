use kaduox_ssh_core::{
    Authentication, ConnectionProgress, JumpAuthFuture, JumpAuthProvider, JumpAuthRequest,
};

#[derive(Debug, Default)]
pub(crate) struct InteractiveJumpAuth;

impl JumpAuthProvider for InteractiveJumpAuth {
    fn authentication<'a>(&'a mut self, request: JumpAuthRequest) -> JumpAuthFuture<'a> {
        Box::pin(async move {
            if request.attempt == 1 {
                return Ok(Some(Authentication::Auto {
                    identity_files: request.jump.identity_files.clone(),
                    passphrase: None,
                }));
            }

            // 先试系统凭据存储（与 desktop/MCP 共用账户名格式），命中即免交互。
            if request.attempt == 2 {
                let mut config = kaduox_ssh_core::ConnectionConfig::new(
                    &request.jump.host,
                    &request.jump.username,
                );
                config.port = request.jump.port;
                let account = kaduox_ssh_core::keyring_account_name(&config);
                if let Ok(entry) = keyring::Entry::new("kssh", &account)
                    && let Ok(password) = entry.get_password()
                    && !password.is_empty()
                {
                    return Ok(Some(Authentication::Password(password)));
                }
            }

            // 非交互环境（管道/脚本/自动化）没有 tty，密码提示会永久挂起；
            // 直接失败并提示改用 --password 或预存凭据。
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                anyhow::bail!(
                    "跳板 {} 需要密码但当前是非交互环境；请用 --password 提供或先在主机库存入凭据",
                    request.jump.alias
                );
            }

            let prompt = format!(
                "Jump {}/{} {} ({}@{}:{}) password, attempt {}: ",
                request.index + 1,
                request.total,
                request.jump.alias,
                request.jump.username,
                request.jump.host,
                request.jump.port,
                request.attempt
            );
            let password =
                tokio::task::spawn_blocking(move || rpassword::prompt_password(prompt)).await??;
            if password.is_empty() {
                return Ok(None);
            }
            Ok(Some(Authentication::Password(password)))
        })
    }
}

pub(crate) async fn print_progress(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<ConnectionProgress>,
) {
    while let Some(event) = receiver.recv().await {
        match event {
            ConnectionProgress::JumpConnecting {
                index,
                total,
                alias,
                host,
                port,
            } => eprintln!(
                "[jump {}/{}] connecting {} -> {}:{}",
                index + 1,
                total,
                terminal_safe(&alias),
                terminal_safe(&host),
                port
            ),
            ConnectionProgress::JumpConnected {
                index,
                total,
                alias,
            } => eprintln!(
                "[jump {}/{}] transport ready: {}",
                index + 1,
                total,
                terminal_safe(&alias)
            ),
            ConnectionProgress::JumpAuthenticating {
                index,
                total,
                alias,
                attempt,
                method,
            } => eprintln!(
                "[jump {}/{}] authenticating {} via {} (attempt {})",
                index + 1,
                total,
                terminal_safe(&alias),
                method.as_str(),
                attempt
            ),
            ConnectionProgress::JumpAuthenticationFailed {
                index,
                total,
                alias,
                attempt,
            } => eprintln!(
                "[jump {}/{}] authentication failed for {} (attempt {})",
                index + 1,
                total,
                terminal_safe(&alias),
                attempt
            ),
            ConnectionProgress::JumpAuthenticated {
                index,
                total,
                alias,
                method,
            } => eprintln!(
                "[jump {}/{}] authenticated {} via {}",
                index + 1,
                total,
                terminal_safe(&alias),
                method.as_str()
            ),
            ConnectionProgress::FinalConnecting { alias, host, port } => eprintln!(
                "[target] connecting {} -> {}:{}",
                terminal_safe(&alias),
                terminal_safe(&host),
                port
            ),
            ConnectionProgress::FinalAuthenticating {
                alias,
                user,
                method,
            } => eprintln!(
                "[target] authenticating {} as {} via {}",
                terminal_safe(&alias),
                terminal_safe(&user),
                method.as_str()
            ),
            ConnectionProgress::Connected { alias } => {
                eprintln!("[target] connected {}", terminal_safe(&alias));
            }
        }
    }
}

fn terminal_safe(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { '�' } else { ch })
        .collect()
}
