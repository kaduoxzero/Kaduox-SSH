//! AI 对话与命令审计的 SQLite 持久化。
//! 数据库文件位于主机库目录下的 `ai-chat.db`（Windows: %APPDATA%\Kaduox-SSH\）。

use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::util::now_unix;

const MAX_TITLE_CHARS: usize = 200;
const MAX_MESSAGE_CHARS: usize = 64 * 1024;
const MAX_MESSAGES_PER_CONVERSATION: usize = 2_000;
const MAX_CONVERSATIONS: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConversation {
    pub id: String,
    pub title: String,
    pub created_at_unix: u64,
    pub updated_at_unix: u64,
    pub message_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStoredMessage {
    pub id: i64,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    /// OpenAI tool_calls JSON 或 tool 结果的附加信息；无附加信息时为 None。
    pub tool_json: Option<String>,
    pub created_at_unix: u64,
}

/// command_audit 表结构文档（当前只写不读，读取接口留给后续审计视图）。
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCommandAudit {
    pub id: i64,
    pub alias: String,
    pub command: String,
    pub risk_level: String,
    pub permission_mode: String,
    pub approved_by_user: bool,
    pub exit_status: Option<i64>,
    pub duration_ms: i64,
    pub created_at_unix: u64,
}

fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).context("打开 AI 对话数据库失败")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS conversations (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            created_at_unix INTEGER NOT NULL,
            updated_at_unix INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            tool_json TEXT,
            created_at_unix INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_messages_conversation
            ON messages(conversation_id, id);
        CREATE TABLE IF NOT EXISTS command_audit (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            alias TEXT NOT NULL,
            command TEXT NOT NULL,
            risk_level TEXT NOT NULL,
            permission_mode TEXT NOT NULL,
            approved_by_user INTEGER NOT NULL,
            exit_status INTEGER,
            duration_ms INTEGER NOT NULL,
            created_at_unix INTEGER NOT NULL
        );
        ",
    )?;
    Ok(conn)
}

fn validate_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
        bail!("会话标题长度必须在 1..={MAX_TITLE_CHARS} 字符之间");
    }
    Ok(title.to_owned())
}

pub fn create_conversation(path: &Path, title: &str) -> Result<AiConversation> {
    let title = validate_title(title)?;
    let conn = open(path)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM conversations", [], |r| r.get(0))?;
    if count as usize >= MAX_CONVERSATIONS {
        bail!("AI 会话数量已达上限 {MAX_CONVERSATIONS}，请先删除旧会话");
    }
    let now = now_unix()?;
    let id = format!("conv-{now}-{}", rand_suffix());
    conn.execute(
        "INSERT INTO conversations (id, title, created_at_unix, updated_at_unix) VALUES (?1, ?2, ?3, ?3)",
        params![id, title, now],
    )?;
    Ok(AiConversation {
        id,
        title,
        created_at_unix: now,
        updated_at_unix: now,
        message_count: 0,
    })
}

fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

pub fn list_conversations(path: &Path) -> Result<Vec<AiConversation>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let conn = open(path)?;
    let mut stmt = conn.prepare(
        "SELECT c.id, c.title, c.created_at_unix, c.updated_at_unix, COUNT(m.id)
         FROM conversations c LEFT JOIN messages m ON m.conversation_id = c.id
         GROUP BY c.id ORDER BY c.updated_at_unix DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AiConversation {
            id: row.get(0)?,
            title: row.get(1)?,
            created_at_unix: row.get(2)?,
            updated_at_unix: row.get(3)?,
            message_count: row.get(4)?,
        })
    })?;
    let mut output = Vec::new();
    for row in rows {
        output.push(row?);
    }
    Ok(output)
}

pub fn rename_conversation(path: &Path, id: &str, title: &str) -> Result<()> {
    let title = validate_title(title)?;
    let conn = open(path)?;
    let changed = conn.execute(
        "UPDATE conversations SET title = ?2, updated_at_unix = ?3 WHERE id = ?1",
        params![id, title, now_unix()?],
    )?;
    if changed == 0 {
        bail!("会话不存在");
    }
    Ok(())
}

pub fn delete_conversation(path: &Path, id: &str) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let conn = open(path)?;
    conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn list_messages(path: &Path, conversation_id: &str) -> Result<Vec<AiStoredMessage>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let conn = open(path)?;
    let mut stmt = conn.prepare(
        "SELECT id, conversation_id, role, content, tool_json, created_at_unix
         FROM messages WHERE conversation_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![conversation_id], |row| {
        Ok(AiStoredMessage {
            id: row.get(0)?,
            conversation_id: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            tool_json: row.get(4)?,
            created_at_unix: row.get(5)?,
        })
    })?;
    let mut output = Vec::new();
    for row in rows {
        output.push(row?);
    }
    Ok(output)
}

pub fn append_message(
    path: &Path,
    conversation_id: &str,
    role: &str,
    content: &str,
    tool_json: Option<&str>,
) -> Result<AiStoredMessage> {
    if !matches!(role, "user" | "assistant" | "tool") {
        bail!("AI 消息角色无效");
    }
    if content.chars().count() > MAX_MESSAGE_CHARS {
        bail!("单条 AI 消息不能超过 {MAX_MESSAGE_CHARS} 字符");
    }
    let conn = open(path)?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1",
        params![conversation_id],
        |r| r.get(0),
    )?;
    if count as usize >= MAX_MESSAGES_PER_CONVERSATION {
        bail!("单个会话的消息数量已达上限 {MAX_MESSAGES_PER_CONVERSATION}");
    }
    let now = now_unix()?;
    conn.execute(
        "INSERT INTO messages (conversation_id, role, content, tool_json, created_at_unix)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![conversation_id, role, content, tool_json, now],
    )?;
    conn.execute(
        "UPDATE conversations SET updated_at_unix = ?2 WHERE id = ?1",
        params![conversation_id, now],
    )?;
    Ok(AiStoredMessage {
        id: conn.last_insert_rowid(),
        conversation_id: conversation_id.to_owned(),
        role: role.to_owned(),
        content: content.to_owned(),
        tool_json: tool_json.map(str::to_owned),
        created_at_unix: now,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn record_command_audit(
    path: &Path,
    alias: &str,
    command: &str,
    risk_level: &str,
    permission_mode: &str,
    approved_by_user: bool,
    exit_status: Option<i64>,
    duration_ms: i64,
) -> Result<()> {
    let conn = open(path)?;
    conn.execute(
        "INSERT INTO command_audit
         (alias, command, risk_level, permission_mode, approved_by_user, exit_status, duration_ms, created_at_unix)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            alias,
            command,
            risk_level,
            permission_mode,
            approved_by_user as i64,
            exit_status,
            duration_ms,
            now_unix()?,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai-chat.db");
        (dir, path)
    }

    #[test]
    fn conversation_lifecycle() {
        let (_dir, path) = temp_db();
        let conv = create_conversation(&path, " 排查 nginx ").unwrap();
        assert_eq!(conv.title, "排查 nginx");

        append_message(&path, &conv.id, "user", "hello", None).unwrap();
        append_message(&path, &conv.id, "assistant", "hi", None).unwrap();
        append_message(
            &path,
            &conv.id,
            "tool",
            "exit 0",
            Some(r#"{"name":"execute_command"}"#),
        )
        .unwrap();

        let messages = list_messages(&path, &conv.id).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2].role, "tool");
        assert!(messages[2].tool_json.is_some());

        let list = list_conversations(&path).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].message_count, 3);

        rename_conversation(&path, &conv.id, "新标题").unwrap();
        assert_eq!(list_conversations(&path).unwrap()[0].title, "新标题");

        delete_conversation(&path, &conv.id).unwrap();
        assert!(list_conversations(&path).unwrap().is_empty());
        assert!(list_messages(&path, &conv.id).unwrap().is_empty());
    }

    #[test]
    fn invalid_roles_and_titles_are_rejected() {
        let (_dir, path) = temp_db();
        assert!(create_conversation(&path, "  ").is_err());
        let conv = create_conversation(&path, "ok").unwrap();
        assert!(append_message(&path, &conv.id, "system", "x", None).is_err());
        assert!(rename_conversation(&path, "missing", "x").is_err());
    }

    #[test]
    fn command_audit_roundtrip() {
        let (_dir, path) = temp_db();
        record_command_audit(
            &path, "web-1", "rm -rf /tmp/build", "delete", "approval", true, Some(0), 42,
        )
        .unwrap();
        let conn = open(&path).unwrap();
        let (alias, level, approved): (String, String, i64) = conn
            .query_row(
                "SELECT alias, risk_level, approved_by_user FROM command_audit",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(alias, "web-1");
        assert_eq!(level, "delete");
        assert_eq!(approved, 1);
    }
}
