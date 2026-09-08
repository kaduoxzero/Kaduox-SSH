use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::RemoteUser;
use sysinfo::{Disks, Networks, System};
use tauri::State;
use tokio::time::sleep;

use crate::commands::hosts::{open_store, route_to_dto};
use crate::models::{
    BasicInfoDto, CpuMetricsDto, DiskMetricsDto, GpuMetricsDto, MemoryMetricsDto,
    NetworkMetricsDto, RouteNodeDto, SystemMetricsDto,
};
use crate::state::DesktopState;
use crate::util::now_unix;

const BASIC_INFO_COMMAND: &str = "printf '__KADUOX_BASIC_INFO_V1__\\n'; printf 'hostname='; hostname 2>/dev/null; printf 'platform='; uname -srmo 2>/dev/null; printf 'username='; id -un 2>/dev/null; printf 'uptime='; uptime -p 2>/dev/null || uptime 2>/dev/null; printf 'addresses='; hostname -I 2>/dev/null";
const SYSTEM_METRICS_COMMAND: &str = include_str!("metrics.sh");

#[derive(Default)]
struct ParsedRemoteInfo {
    hostname: String,
    platform: String,
    username: String,
    uptime: String,
    addresses: Vec<String>,
}

fn parse_remote_info(output: &str) -> Result<ParsedRemoteInfo> {
    let mut lines = output.lines();
    if lines.next() != Some("__KADUOX_BASIC_INFO_V1__") {
        bail!("远程主机未返回可识别的基础信息格式");
    }

    let mut info = ParsedRemoteInfo::default();
    for line in lines {
        if let Some(value) = line.strip_prefix("hostname=") {
            info.hostname = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("platform=") {
            info.platform = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("username=") {
            info.username = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("uptime=") {
            info.uptime = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("addresses=") {
            info.addresses = value
                .split_whitespace()
                .map(ToOwned::to_owned)
                .filter(|item| !item.is_empty())
                .collect();
        }
    }
    if info.hostname.is_empty() {
        bail!("远程主机未返回主机名");
    }
    Ok(info)
}

fn local_value(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| "未知".to_owned())
}

#[tauri::command]
pub fn get_local_basic_info() -> Result<BasicInfoDto, String> {
    Ok(BasicInfoDto {
        scope: "local".to_owned(),
        hostname: System::host_name().unwrap_or_else(|| {
            std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "本机".to_owned())
        }),
        platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        username: local_value("USERNAME"),
        uptime: format_uptime(System::uptime()),
        addresses: Vec::new(),
        route: Vec::new(),
        queried_at_unix: now_unix().map_err(|error| error.to_string())?,
    })
}

#[tauri::command]
pub async fn query_basic_info(
    alias: String,
    state: State<'_, DesktopState>,
) -> Result<BasicInfoDto, String> {
    query_basic_info_inner(alias, &state)
        .await
        .map_err(|error| error.to_string())
}

async fn query_basic_info_inner(alias: String, state: &DesktopState) -> Result<BasicInfoDto> {
    let alias = alias.trim();
    if alias.is_empty() {
        bail!("请选择要查询的主机");
    }

    let config = {
        let _guard = state.host_store_guard.lock().await;
        let store = open_store()?;
        kaduox_ssh_hosts::resolve_host(store.database(), alias, None, None)
            .with_context(|| format!("无法解析主机 {alias}"))?
            .config
    };
    let lease = state
        .session_lease(alias)
        .await
        .map_err(anyhow::Error::msg)?;

    let output = lease
        .exec(BASIC_INFO_COMMAND, &RemoteUser::Current)
        .await
        .context("无法执行基础信息查询")?;
    let raw = String::from_utf8_lossy(&output.stdout);
    let parsed = parse_remote_info(&raw)?;
    Ok(BasicInfoDto {
        scope: "remote".to_owned(),
        hostname: parsed.hostname,
        platform: parsed.platform,
        username: parsed.username,
        uptime: parsed.uptime,
        addresses: parsed.addresses,
        route: route_to_dto(&config),
        queried_at_unix: now_unix()?,
    })
}

#[derive(Default)]
struct ParsedSystemMetrics {
    hostname: String,
    platform: String,
    username: String,
    uptime: String,
    addresses: Vec<String>,
    cpu_usage: Option<f32>,
    gpus: Vec<GpuMetricsDto>,
    cpu_model: String,
    cpu_cores: Option<u32>,
    load_1: Option<f32>,
    load_5: Option<f32>,
    load_15: Option<f32>,
    memory: Option<(u64, u64, u64)>,
    disk: Option<(u64, u64, u64, f32)>,
    gpu_name: String,
    gpu_usage: Option<f32>,
    gpu_memory: Option<(u64, u64)>,
    gpu_temperature: Option<f32>,
    network: Option<(u64, u64)>,
}

fn parse_u64(value: &str) -> Option<u64> {
    value.trim().trim_end_matches(',').parse().ok()
}

fn parse_f32(value: &str) -> Option<f32> {
    let value = value.trim().trim_end_matches('%').trim_end_matches(',');
    let value = value.parse::<f32>().ok()?;
    value.is_finite().then_some(value)
}

fn parse_u64_triplet(value: &str) -> Option<(u64, u64, u64)> {
    let values = value
        .split_whitespace()
        .filter_map(parse_u64)
        .collect::<Vec<_>>();
    (values.len() >= 3).then(|| (values[0], values[1], values[2]))
}

fn parse_u64_pair(value: &str) -> Option<(u64, u64)> {
    let values = value
        .split_whitespace()
        .filter_map(parse_u64)
        .collect::<Vec<_>>();
    (values.len() >= 2).then(|| (values[0], values[1]))
}

fn parse_f32_triplet(value: &str) -> [Option<f32>; 3] {
    let mut values = value.split_whitespace().map(parse_f32);
    [
        values.next().flatten(),
        values.next().flatten(),
        values.next().flatten(),
    ]
}

fn parse_system_metrics(output: &str) -> Result<ParsedSystemMetrics> {
    let mut lines = output.lines();
    if lines.next() != Some("__KADUOX_SYSTEM_METRICS_V1__") {
        bail!("远程主机未返回可识别的系统监控格式");
    }

    let mut metrics = ParsedSystemMetrics::default();
    for line in lines {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "hostname" => metrics.hostname = value.to_owned(),
            "platform" => metrics.platform = value.to_owned(),
            "username" => metrics.username = value.to_owned(),
            "uptime" => metrics.uptime = value.to_owned(),
            "addresses" => {
                metrics.addresses = value
                    .split_whitespace()
                    .filter(|item| !item.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
            }
            "cpu_usage" => metrics.cpu_usage = parse_f32(value),
            "gpu" => {
                let fields: Vec<_> = value.split(',').map(str::trim).collect();
                if fields.len() == 5 {
                    metrics.gpus.push(GpuMetricsDto {
                        name: fields[0].to_owned(),
                        usage_percent: parse_f32(fields[1]),
                        memory_used_bytes: parse_u64(fields[2])
                            .map(|n| n.saturating_mul(1024 * 1024)),
                        memory_total_bytes: parse_u64(fields[3])
                            .map(|n| n.saturating_mul(1024 * 1024)),
                        temperature_c: parse_f32(fields[4]),
                    });
                }
            }
            "cpu_model" => metrics.cpu_model = value.to_owned(),
            "cpu_cores" => {
                metrics.cpu_cores = parse_u64(value).and_then(|item| u32::try_from(item).ok());
            }
            "load" => {
                let [load_1, load_5, load_15] = parse_f32_triplet(value);
                metrics.load_1 = load_1;
                metrics.load_5 = load_5;
                metrics.load_15 = load_15;
            }
            "memory" => metrics.memory = parse_u64_triplet(value),
            "disk" => {
                let values = value.split_whitespace().collect::<Vec<_>>();
                if values.len() >= 4
                    && let (Some(total), Some(used), Some(available), Some(usage)) = (
                        parse_u64(values[0]),
                        parse_u64(values[1]),
                        parse_u64(values[2]),
                        parse_f32(values[3]),
                    )
                {
                    metrics.disk = Some((total, used, available, usage));
                }
            }
            "gpu_name" => metrics.gpu_name = value.to_owned(),
            "gpu_usage" => metrics.gpu_usage = parse_f32(value),
            "gpu_memory" => metrics.gpu_memory = parse_u64_pair(value),
            "gpu_temperature" => metrics.gpu_temperature = parse_f32(value),
            "network" => metrics.network = parse_u64_pair(value),
            _ => {}
        }
    }
    if metrics.hostname.is_empty() {
        bail!("远程主机未返回主机名");
    }
    Ok(metrics)
}

fn metrics_to_dto(
    scope: &str,
    route: Vec<RouteNodeDto>,
    metrics: ParsedSystemMetrics,
    queried_at_unix: u64,
) -> SystemMetricsDto {
    let usage_percent = metrics.cpu_usage.map(|value| value.clamp(0.0, 100.0));
    let memory = metrics
        .memory
        .map(|(total, used, available)| MemoryMetricsDto {
            total_bytes: Some(total),
            used_bytes: Some(used.min(total)),
            available_bytes: Some(available),
            usage_percent: (total > 0)
                .then_some((used as f32 / total as f32 * 100.0).clamp(0.0, 100.0)),
        })
        .unwrap_or(MemoryMetricsDto {
            total_bytes: None,
            used_bytes: None,
            available_bytes: None,
            usage_percent: None,
        });
    let disks = metrics
        .disk
        .map(|(total, used, available, usage)| {
            vec![DiskMetricsDto {
                mount: "/".to_owned(),
                total_bytes: Some(total),
                used_bytes: Some(used),
                available_bytes: Some(available),
                usage_percent: Some(usage.clamp(0.0, 100.0)),
            }]
        })
        .unwrap_or_default();
    let gpus = if !metrics.gpus.is_empty() {
        metrics.gpus
    } else if metrics.gpu_name.is_empty() {
        Vec::new()
    } else {
        vec![GpuMetricsDto {
            name: metrics.gpu_name,
            usage_percent: metrics.gpu_usage.map(|value| value.clamp(0.0, 100.0)),
            memory_used_bytes: metrics
                .gpu_memory
                .map(|(used, _)| used.saturating_mul(1024 * 1024)),
            memory_total_bytes: metrics
                .gpu_memory
                .map(|(_, total)| total.saturating_mul(1024 * 1024)),
            temperature_c: metrics.gpu_temperature,
        }]
    };
    SystemMetricsDto {
        scope: scope.to_owned(),
        hostname: metrics.hostname,
        platform: metrics.platform,
        username: metrics.username,
        uptime: metrics.uptime,
        addresses: metrics.addresses,
        route,
        queried_at_unix,
        cpu: CpuMetricsDto {
            model: if metrics.cpu_model.is_empty() {
                "未知 CPU".to_owned()
            } else {
                metrics.cpu_model
            },
            cores: metrics.cpu_cores,
            usage_percent,
            load_1: metrics.load_1,
            load_5: metrics.load_5,
            load_15: metrics.load_15,
        },
        memory,
        disks,
        gpus,
        network: NetworkMetricsDto {
            rx_bytes: metrics.network.map(|(rx, _)| rx),
            tx_bytes: metrics.network.map(|(_, tx)| tx),
        },
    }
}

fn local_fallback_metrics() -> ParsedSystemMetrics {
    ParsedSystemMetrics {
        hostname: System::host_name().unwrap_or_else(|| {
            std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "本机".to_owned())
        }),
        platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        username: local_value("USERNAME"),
        uptime: "当前客户端主机".to_owned(),
        cpu_model: std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "未知 CPU".to_owned()),
        cpu_cores: std::thread::available_parallelism()
            .ok()
            .map(|value| value.get() as u32),
        ..ParsedSystemMetrics::default()
    }
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("up {days} days, {hours} hours")
    } else if hours > 0 {
        format!("up {hours} hours, {minutes} minutes")
    } else {
        format!("up {minutes} minutes")
    }
}

/// Query `nvidia-smi` on the local machine for GPU telemetry.
///
/// `nvidia-smi` reports memory in MiB with `nounits`; values are converted to
/// bytes so the frontend byte formatters stay correct. Returns an empty list
/// when no NVIDIA driver is installed — the UI renders its empty state.
fn local_gpus() -> Vec<GpuMetricsDto> {
    let query = [
        "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
        "--format=csv,noheader,nounits",
    ];

    let mut command = std::process::Command::new("nvidia-smi");
    command.args(query);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = match command.output() {
        Ok(output) if output.status.success() => output,
        _ => {
            // The driver usually places nvidia-smi in System32; try the
            // absolute path when it is not on PATH.
            let mut fallback = std::process::Command::new(
                PathBuf::from(r"C:\Windows\System32").join("nvidia-smi.exe"),
            );
            fallback.args(query);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                fallback.creation_flags(CREATE_NO_WINDOW);
            }
            match fallback.output() {
                Ok(output) if output.status.success() => output,
                _ => return Vec::new(),
            }
        }
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
            if fields.len() < 5 {
                return None;
            }
            let name = fields[0];
            if name.is_empty() {
                return None;
            }
            let memory_mib_to_bytes =
                |value: &str| parse_u64(value).map(|mib| mib.saturating_mul(1024 * 1024));
            Some(GpuMetricsDto {
                name: name.to_owned(),
                usage_percent: parse_f32(fields[1]).map(|value| value.clamp(0.0, 100.0)),
                memory_used_bytes: memory_mib_to_bytes(fields[2]),
                memory_total_bytes: memory_mib_to_bytes(fields[3]),
                temperature_c: parse_f32(fields[4]),
            })
        })
        .collect()
}

/// Collect the local Windows client's own resource snapshot.
///
/// CPU usage needs two samples at least [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`]
/// apart, so the command is async and sleeps between refreshes. Windows has no
/// Unix load average, so load fields stay `None` and the ring uses the measured
/// usage percent directly.
#[tauri::command]
pub async fn get_local_system_metrics() -> Result<SystemMetricsDto, String> {
    local_system_metrics()
        .await
        .map_err(|error| error.to_string())
}

async fn local_system_metrics() -> Result<SystemMetricsDto> {
    let mut fallback = local_fallback_metrics();

    let mut system = System::new();
    system.refresh_memory();
    system.refresh_cpu_usage();
    sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    system.refresh_cpu_usage();

    if let Some(cpu) = system.cpus().first() {
        let brand = cpu.brand().trim();
        if !brand.is_empty() {
            fallback.cpu_model = brand.to_owned();
        }
    }
    let cores = u32::try_from(system.cpus().len()).unwrap_or(0);
    if cores > 0 {
        fallback.cpu_cores = Some(cores);
    }
    let measured_usage = system.global_cpu_usage();
    let cpu_usage = measured_usage
        .is_finite()
        .then(|| measured_usage.clamp(0.0, 100.0));

    let memory = {
        let total = system.total_memory();
        (total > 0).then(|| {
            let used = system.used_memory().min(total);
            MemoryMetricsDto {
                total_bytes: Some(total),
                used_bytes: Some(used),
                available_bytes: Some(system.available_memory()),
                usage_percent: Some((used as f32 / total as f32 * 100.0).clamp(0.0, 100.0)),
            }
        })
    };

    let disks = local_system_disk(&Disks::new_with_refreshed_list())
        .map(|(mount, total, available)| {
            let used = total.saturating_sub(available);
            DiskMetricsDto {
                mount,
                total_bytes: Some(total),
                used_bytes: Some(used),
                available_bytes: Some(available),
                usage_percent: (total > 0)
                    .then_some((used as f32 / total as f32 * 100.0).clamp(0.0, 100.0)),
            }
        })
        .into_iter()
        .collect::<Vec<_>>();

    let (rx_bytes, tx_bytes) = local_network_totals(&Networks::new_with_refreshed_list());

    Ok(SystemMetricsDto {
        scope: "local".to_owned(),
        hostname: fallback.hostname,
        platform: fallback.platform,
        username: fallback.username,
        uptime: format_uptime(System::uptime()),
        addresses: fallback.addresses,
        route: Vec::new(),
        queried_at_unix: now_unix()?,
        cpu: CpuMetricsDto {
            model: if fallback.cpu_model.is_empty() {
                "未知 CPU".to_owned()
            } else {
                fallback.cpu_model
            },
            cores: fallback.cpu_cores,
            usage_percent: cpu_usage,
            load_1: None,
            load_5: None,
            load_15: None,
        },
        memory: memory.unwrap_or(MemoryMetricsDto {
            total_bytes: None,
            used_bytes: None,
            available_bytes: None,
            usage_percent: None,
        }),
        disks,
        gpus: local_gpus(),
        network: NetworkMetricsDto {
            rx_bytes: Some(rx_bytes),
            tx_bytes: Some(tx_bytes),
        },
    })
}

/// Locate the Windows system drive and return `(mount, total, available)`.
fn local_system_disk(disks: &Disks) -> Option<(String, u64, u64)> {
    let system_drive = std::env::var_os("SystemDrive").map(PathBuf::from);
    let disk = disks
        .iter()
        .find(|disk| {
            system_drive
                .as_deref()
                .is_some_and(|drive| disk.mount_point().starts_with(drive))
        })
        .or_else(|| {
            disks
                .iter()
                .find(|disk| disk.mount_point() == Path::new("/"))
        })?;
    let total = disk.total_space();
    let available = disk.available_space().min(total);
    Some((
        disk.mount_point().to_string_lossy().into_owned(),
        total,
        available,
    ))
}

/// Sum received/transmitted bytes across non-loopback interfaces since boot.
fn local_network_totals(networks: &Networks) -> (u64, u64) {
    let mut rx = 0_u64;
    let mut tx = 0_u64;
    for (name, data) in networks {
        if name.to_ascii_lowercase().contains("loopback") {
            continue;
        }
        rx = rx.saturating_add(data.total_received());
        tx = tx.saturating_add(data.total_transmitted());
    }
    (rx, tx)
}

#[tauri::command]
pub async fn query_system_metrics(
    alias: String,
    state: State<'_, DesktopState>,
) -> Result<SystemMetricsDto, String> {
    query_system_metrics_inner(alias, &state)
        .await
        .map_err(|error| error.to_string())
}

async fn query_system_metrics_inner(
    alias: String,
    state: &DesktopState,
) -> Result<SystemMetricsDto> {
    let alias = alias.trim();
    if alias.is_empty() {
        bail!("请选择要查询的主机");
    }
    let lease = state
        .session_lease(alias)
        .await
        .map_err(anyhow::Error::msg)?;
    let mut stdout = super::connection::CappedWriter::default();
    let mut stderr = super::connection::CappedWriter::default();
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        lease.exec_stream(
            SYSTEM_METRICS_COMMAND,
            &RemoteUser::Current,
            &mut stdout,
            &mut stderr,
        ),
    )
    .await
    .context("指标采集超过 8 秒，下个周期将重试")??;
    anyhow::ensure!(
        status == Some(0),
        "指标采集命令失败，目标需要 Linux /proc 支持"
    );
    let (stdout, truncated) = stdout.into_parts();
    anyhow::ensure!(!truncated, "指标输出超过大小限制");
    let metrics = parse_system_metrics(&String::from_utf8_lossy(&stdout))?;
    Ok(metrics_to_dto(
        "remote",
        route_to_dto(lease.config()),
        metrics,
        now_unix()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RouteNodeDto;

    #[test]
    fn parses_keyed_remote_info() {
        let parsed = parse_remote_info(
            "__KADUOX_BASIC_INFO_V1__\nhostname=lab\nplatform=Linux 6.8 x86_64\nusername=root\nuptime=up 2 hours\naddresses=10.0.0.2 127.0.0.1\n",
        )
        .unwrap();
        assert_eq!(parsed.hostname, "lab");
        assert_eq!(parsed.addresses, vec!["10.0.0.2", "127.0.0.1"]);
    }

    #[test]
    fn rejects_unknown_marker() {
        assert!(parse_remote_info("hostname=lab\n").is_err());
    }

    #[test]
    fn parses_measured_cpu_large_network_totals_and_multiple_gpus() {
        let parsed = parse_system_metrics("__KADUOX_SYSTEM_METRICS_V1__\nhostname=lab\ncpu_usage=25.5\ncpu_cores=8\nload=8 8 8\ngpu=A4000, 17, 512, 16384, 54\ngpu=T4, 0, 0, 16384, 40\nnetwork=2000000000000 400000000000\n").unwrap();
        let metrics = metrics_to_dto("remote", vec![], parsed, 1);
        assert_eq!(metrics.cpu.usage_percent, Some(25.5)); // not load / cores
        assert_eq!(metrics.gpus.len(), 2);
        assert_eq!(metrics.gpus[0].memory_used_bytes, Some(512 * 1024 * 1024));
        assert_eq!(metrics.network.rx_bytes, Some(2_000_000_000_000));
    }

    #[test]
    fn route_node_dto_is_serializable_shape() {
        let node = RouteNodeDto {
            alias: "edge".into(),
            host: "192.0.2.1".into(),
            port: 22,
            username: "jump".into(),
            role: "jump".into(),
        };
        assert_eq!(node.role, "jump");
    }
}
