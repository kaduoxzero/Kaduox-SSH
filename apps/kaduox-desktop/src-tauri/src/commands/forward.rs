use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{DynamicForward, LocalForward, RemoteForward};
use tauri::State;

use crate::models::{ForwardDto, ForwardStartRequest};
use crate::state::{DesktopState, ForwardControl, ForwardResource};

#[cfg(test)]
#[path = "forward_live_test.rs"]
mod live_tests;

fn parse_bind(address: &str, port: u16) -> Result<SocketAddr> {
    let address: IpAddr = address
        .trim()
        .parse()
        .with_context(|| format!("无效的监听 IP 地址 {address:?}"))?;
    Ok(SocketAddr::new(address, port))
}

fn validate_target(host: &str, port: u16) -> Result<()> {
    if host.is_empty() || host.len() > 512 || host.chars().any(char::is_control) {
        bail!("转发目标主机无效");
    }
    if port == 0 {
        bail!("转发目标端口不能为 0");
    }
    Ok(())
}

#[tauri::command]
pub async fn start_forward(
    request: ForwardStartRequest,
    state: State<'_, DesktopState>,
) -> Result<ForwardDto, String> {
    start_forward_inner(request, &state)
        .await
        .map_err(|error| error.to_string())
}

async fn start_forward_inner(
    request: ForwardStartRequest,
    state: &DesktopState,
) -> Result<ForwardDto> {
    let id = state.next_id("forward");
    let (details, resource, lease) = match request {
        ForwardStartRequest::Local {
            alias,
            bind_address,
            bind_port,
            target_host,
            target_port,
        } => {
            validate_target(&target_host, target_port)?;
            let lease = state
                .session_lease(alias.trim())
                .await
                .map_err(anyhow::Error::msg)?;
            let handle = lease
                .local_forward(LocalForward {
                    bind: parse_bind(&bind_address, bind_port)?,
                    target_host: target_host.clone(),
                    target_port,
                })
                .await?;
            let actual = handle.bound_addr();
            (
                ForwardDto {
                    route: super::hosts::route_to_dto(lease.config()),
                    id: id.clone(),
                    alias,
                    kind: "local".to_owned(),
                    bind_address: actual.ip().to_string(),
                    bind_port: actual.port(),
                    target_host: Some(target_host),
                    target_port: Some(target_port),
                },
                ForwardResource::Local(handle),
                lease,
            )
        }
        ForwardStartRequest::Dynamic {
            alias,
            bind_address,
            bind_port,
        } => {
            let lease = state
                .session_lease(alias.trim())
                .await
                .map_err(anyhow::Error::msg)?;
            let handle = lease
                .dynamic_forward(DynamicForward {
                    bind: parse_bind(&bind_address, bind_port)?,
                })
                .await?;
            let actual = handle.bound_addr();
            (
                ForwardDto {
                    route: super::hosts::route_to_dto(lease.config()),
                    id: id.clone(),
                    alias,
                    kind: "dynamic".to_owned(),
                    bind_address: actual.ip().to_string(),
                    bind_port: actual.port(),
                    target_host: None,
                    target_port: None,
                },
                ForwardResource::Local(handle),
                lease,
            )
        }
        ForwardStartRequest::Remote {
            alias,
            bind_address,
            bind_port,
            target_host,
            target_port,
        } => {
            validate_target(&target_host, target_port)?;
            if bind_address.is_empty()
                || bind_address.len() > 512
                || bind_address.chars().any(char::is_control)
            {
                bail!("远程监听地址无效");
            }
            let lease = state
                .session_lease(alias.trim())
                .await
                .map_err(anyhow::Error::msg)?;
            let handle = lease
                .remote_forward_managed(RemoteForward {
                    bind_address: bind_address.clone(),
                    bind_port,
                    target_host: target_host.clone(),
                    target_port,
                })
                .await?;
            let actual_port = handle.port();
            (
                ForwardDto {
                    route: super::hosts::route_to_dto(lease.config()),
                    id: id.clone(),
                    alias,
                    kind: "remote".to_owned(),
                    bind_address,
                    bind_port: actual_port,
                    target_host: Some(target_host),
                    target_port: Some(target_port),
                },
                ForwardResource::Remote(handle),
                lease,
            )
        }
    };
    state.forwards.write().await.insert(
        id,
        ForwardControl {
            details: details.clone(),
            resource,
            _lease: lease,
        },
    );
    Ok(details)
}

#[tauri::command]
pub async fn list_forwards(state: State<'_, DesktopState>) -> Result<Vec<ForwardDto>, String> {
    let mut forwards = state
        .forwards
        .read()
        .await
        .values()
        .map(|forward| forward.details.clone())
        .collect::<Vec<_>>();
    forwards.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(forwards)
}

#[tauri::command]
pub async fn stop_forward(id: String, state: State<'_, DesktopState>) -> Result<(), String> {
    if let Some(control) = state.forwards.write().await.remove(&id) {
        control
            .resource
            .close()
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub async fn stop_forwards_for_alias(alias: &str, state: &DesktopState) {
    let controls = {
        let mut forwards = state.forwards.write().await;
        let ids = forwards
            .iter()
            .filter(|(_, control)| control.details.alias == alias)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| forwards.remove(&id))
            .collect::<Vec<_>>()
    };
    for control in controls {
        let _ = control.resource.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_parser_accepts_ipv4_and_ipv6() {
        assert_eq!(parse_bind("127.0.0.1", 8080).unwrap().port(), 8080);
        assert!(parse_bind("::1", 0).is_ok());
        assert!(parse_bind("localhost", 8080).is_err());
    }
}
