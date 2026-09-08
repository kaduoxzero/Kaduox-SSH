use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::models::HistoryEntryDto;
use anyhow::{Context, Result};
use serde::Serialize;

pub const PAGE_SIZE: usize = 50;

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

pub fn page(path: &Path, requested: usize) -> Result<HistoryPage> {
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
    // Count without loading the whole history into memory. Only deserialize one page.
    let total = BufReader::new(file)
        .lines()
        .try_fold(0usize, |n, line| line.map(|_| n + 1))?;
    let page = requested.max(1).min(total.div_ceil(PAGE_SIZE).max(1));
    let end = total.saturating_sub((page - 1) * PAGE_SIZE);
    let start = end.saturating_sub(PAGE_SIZE);
    let mut entries = Vec::with_capacity(PAGE_SIZE);
    for line in BufReader::new(File::open(path)?)
        .lines()
        .skip(start)
        .take(end - start)
    {
        entries.push(
            serde_json::from_str(&line?).context("运行记录损坏；原文件已保留，请备份后检查")?,
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
    fn persists_across_reopen_and_pages_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        for n in 0..123 {
            append(
                &path,
                &HistoryEntryDto {
                    id: n.to_string(),
                    alias: "test".into(),
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
        let first = page(&path, 1).unwrap();
        assert_eq!(first.total, 123);
        assert_eq!(first.entries.len(), 50);
        assert_eq!(first.entries[0].id, "122");
        assert_eq!(page(&path, 2).unwrap().entries[0].id, "72");
        let last = page(&path, usize::MAX).unwrap();
        assert_eq!(last.entries.len(), 23);
        assert_eq!(last.page, 3);
        assert_eq!(last.entries.last().unwrap().id, "0");
    }
}
