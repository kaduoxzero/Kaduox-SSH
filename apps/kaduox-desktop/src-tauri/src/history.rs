use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::models::HistoryEntryDto;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const PAGE_SIZE: usize = 50;
/// 每台主机保留的命令历史上限，避免文件无限增长。
const MAX_COMMANDS_PER_ALIAS: usize = 500;
const MAX_COMMAND_FILE_ENTRIES: usize = 5000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandHistoryEntry {
    pub alias: String,
    pub command: String,
    pub used_at_unix: u64,
    pub source: String,
}

/// 追加命令记录（按 alias+command 去重，重复时只更新时间并移到末尾）。
pub fn append_commands(
    path: &Path,
    alias: &str,
    commands: &[String],
    source: &str,
    used_at_unix: u64,
) -> Result<usize> {
    let mut entries = read_commands(path)?;
    let mut added = 0usize;
    for command in commands {
        let command = command.trim();
        if command.is_empty() {
            continue;
        }
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.alias == alias && entry.command == command)
        {
            existing.used_at_unix = used_at_unix;
            // 移到末尾视为最新。
            let entry = existing.clone();
            entries.retain(|item| !(item.alias == alias && item.command == command));
            entries.push(entry);
            continue;
        }
        entries.push(CommandHistoryEntry {
            alias: alias.to_owned(),
            command: command.to_owned(),
            used_at_unix,
            source: source.to_owned(),
        });
        added += 1;
    }
    // 超出上限时丢弃最旧的记录。
    while entries.len() > MAX_COMMAND_FILE_ENTRIES {
        entries.remove(0);
    }
    let mut per_alias: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut trimmed: Vec<CommandHistoryEntry> = Vec::with_capacity(entries.len());
    for entry in entries.iter().rev() {
        let count = per_alias.entry(entry.alias.clone()).or_insert(0);
        if *count < MAX_COMMANDS_PER_ALIAS {
            *count += 1;
            trimmed.push(entry.clone());
        }
    }
    trimmed.reverse();
    write_commands(path, &trimmed)?;
    Ok(added)
}

/// 读取某主机的命令历史，最新在前。只保留本软件产生的记录（local/terminal）。
pub fn list_commands(path: &Path, alias: &str, limit: usize) -> Result<Vec<String>> {
    let mut output = Vec::new();
    for entry in read_commands(path)?.iter().rev() {
        if entry.alias == alias && entry.source != "remote" {
            output.push(entry.command.clone());
            if output.len() >= limit {
                break;
            }
        }
    }
    Ok(output)
}

/// 一次性清理早期版本从远端 shell 历史同步进来的记录。
pub fn drop_remote_sources(path: &Path) -> Result<()> {
    let entries = read_commands(path)?;
    if !entries.iter().any(|entry| entry.source == "remote") {
        return Ok(());
    }
    let kept: Vec<_> = entries
        .into_iter()
        .filter(|entry| entry.source != "remote")
        .collect();
    write_commands(path, &kept)
}

fn read_commands(path: &Path) -> Result<Vec<CommandHistoryEntry>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<CommandHistoryEntry>(&line) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn write_commands(path: &Path, entries: &[CommandHistoryEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).context("无法写入命令历史")?;
    for entry in entries {
        file.write_all(serde_json::to_vec(entry)?.as_slice())?;
        file.write_all(b"\n")?;
    }
    file.sync_data().context("命令历史未能落盘")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntryDto>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}

pub fn append(path: &Path, entry: &HistoryEntryDto) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).context("无法打开运行记录")?;
    let mut bytes = serde_json::to_vec(entry)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_data().context("运行记录未能落盘")
}

/// 分页读取运行记录；`day_range` 为 [当天起始, 次日起始) 的 Unix 秒区间（由前端按本地时区计算）。
pub fn page(path: &Path, requested: usize, day_range: Option<(u64, u64)>) -> Result<HistoryPage> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HistoryPage {
                entries: vec![],
                total: 0,
                page: 1,
                page_size: PAGE_SIZE,
            });
        }
        Err(error) => return Err(error.into()),
    };
    let in_range = |line: &str| -> bool {
        let Some((start, end)) = day_range else { return true };
        match serde_json::from_str::<HistoryEntryDto>(line) {
            Ok(entry) => entry.started_at_unix >= start && entry.started_at_unix < end,
            Err(_) => false,
        }
    };
    // Count without loading the whole history into memory. Only deserialize one page.
    let total = BufReader::new(file)
        .lines()
        .try_fold(0usize, |n, line| line.map(|line| n + usize::from(in_range(&line))))?;
    let page = requested.max(1).min(total.div_ceil(PAGE_SIZE).max(1));
    let end = total.saturating_sub((page - 1) * PAGE_SIZE);
    let start = end.saturating_sub(PAGE_SIZE);
    let mut entries = Vec::with_capacity(PAGE_SIZE);
    for line in BufReader::new(File::open(path)?)
        .lines()
        .map_while(|line| line.ok())
        .filter(|line| in_range(line))
        .skip(start)
        .take(end - start)
    {
        entries.push(
            serde_json::from_str(&line).context("运行记录损坏；原文件已保留，请备份后检查")?,
        );
    }
    entries.reverse();
    Ok(HistoryPage {
        entries,
        total,
        page,
        page_size: PAGE_SIZE,
    })
}

/// 从持久化运行记录中提取去重后的命令列表（最新在前），供终端旁的命令历史使用。
pub fn commands(path: &Path, alias: Option<&str>, limit: usize) -> Result<Vec<String>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut seen = std::collections::HashSet::new();
    let mut output = Vec::new();
    let lines: Vec<String> = BufReader::new(file).lines().collect::<std::io::Result<_>>()?;
    for line in lines.iter().rev() {
        let entry: HistoryEntryDto = match serde_json::from_str(line) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if let Some(alias) = alias {
            if entry.alias != alias {
                continue;
            }
        }
        if seen.insert(entry.command.clone()) {
            output.push(entry.command);
            if output.len() >= limit {
                break;
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_history_dedups_and_orders_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commands.jsonl");
        append_commands(&path, "web", &["ls".into(), "pwd".into()], "terminal", 1).unwrap();
        append_commands(&path, "db", &["top".into()], "terminal", 2).unwrap();
        let added = append_commands(&path, "web", &["ls".into(), "git status".into()], "local", 3).unwrap();
        assert_eq!(added, 1);
        assert_eq!(list_commands(&path, "web", 10).unwrap(), ["git status", "ls", "pwd"]);
        assert_eq!(list_commands(&path, "db", 10).unwrap(), ["top"]);
        assert!(list_commands(&path, "none", 10).unwrap().is_empty());
    }
    #[test]
    fn persists_across_reopen_and_pages_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        for n in 0..123 {
            append(
                &path,
                &HistoryEntryDto {
                    id: n.to_string(),
                    alias: "test".into(),
                    username: Some("demo".into()),
                    source: None,
                    command: "printf hello".into(),
                    exit_status: Some(0),
                    succeeded: true,
                    output_preview: "hello\nworld".into(),
                    started_at_unix: n,
                    duration_ms: 1,
                },
            )
            .unwrap();
        }
        let first = page(&path, 1, None).unwrap();
        assert_eq!(first.total, 123);
        assert_eq!(first.entries.len(), 50);
        assert_eq!(first.entries[0].id, "122");
        assert_eq!(page(&path, 2, None).unwrap().entries[0].id, "72");
        let last = page(&path, usize::MAX, None).unwrap();
        assert_eq!(last.entries.len(), 23);
        assert_eq!(last.page, 3);
        assert_eq!(last.entries.last().unwrap().id, "0");
    }

    #[test]
    fn day_range_filters_entries_and_pages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        // 两天各 3 条：第 1 天 [0,10) 秒，第 2 天 [10,20) 秒。
        for n in 0..6u64 {
            append(
                &path,
                &HistoryEntryDto {
                    id: n.to_string(),
                    alias: "test".into(),
                    username: None,
                    source: None,
                    command: format!("cmd-{n}"),
                    exit_status: Some(0),
                    succeeded: true,
                    output_preview: String::new(),
                    started_at_unix: if n < 3 { n } else { 10 + n },
                    duration_ms: 1,
                },
            )
            .unwrap();
        }
        let day1 = page(&path, 1, Some((0, 10))).unwrap();
        assert_eq!(day1.total, 3);
        assert_eq!(day1.entries[0].id, "2");
        let day2 = page(&path, 1, Some((10, 20))).unwrap();
        assert_eq!(day2.total, 3);
        assert_eq!(day2.entries[0].id, "5");
        let empty = page(&path, 1, Some((100, 200))).unwrap();
        assert_eq!(empty.total, 0);
        assert!(empty.entries.is_empty());
        // 不带过滤时行为不变。
        assert_eq!(page(&path, 1, None).unwrap().total, 6);
    }
}
