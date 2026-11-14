//! Хранилище на SQLite: пользователи, сессии, серверы, каналы, сообщения.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

pub type DbResult<T> = rusqlite::Result<T>;

#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub color: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Server {
    pub id: i64,
    pub name: String,
    pub owner_id: i64,
    pub invite: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Channel {
    pub id: i64,
    pub server_id: i64,
    pub name: String,
    pub topic: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Message {
    pub id: i64,
    pub channel_id: i64,
    pub author: User,
    pub content: String,
    pub created_at: i64,
    pub edited_at: Option<i64>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn open(path: &str) -> DbResult<Connection> {
    let c = Connection::open(path)?;
    c.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS users (
             id            INTEGER PRIMARY KEY,
             username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
             password_hash TEXT NOT NULL,
             color         TEXT NOT NULL,
             created_at    INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS sessions (
             token      TEXT PRIMARY KEY,
             user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS servers (
             id         INTEGER PRIMARY KEY,
             name       TEXT NOT NULL,
             owner_id   INTEGER NOT NULL REFERENCES users(id),
             invite     TEXT NOT NULL UNIQUE,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS members (
             server_id INTEGER NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
             user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
             joined_at INTEGER NOT NULL,
             PRIMARY KEY (server_id, user_id)
         );
         CREATE TABLE IF NOT EXISTS channels (
             id         INTEGER PRIMARY KEY,
             server_id  INTEGER NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
             name       TEXT NOT NULL,
             topic      TEXT NOT NULL DEFAULT '',
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS messages (
             id         INTEGER PRIMARY KEY,
             channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
             author_id  INTEGER NOT NULL REFERENCES users(id),
             content    TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             edited_at  INTEGER
         );
         CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel_id, id);
         CREATE INDEX IF NOT EXISTS idx_members_user ON members(user_id);",
    )?;
    Ok(c)
}

fn user_row(r: &Row) -> DbResult<User> {
    Ok(User { id: r.get(0)?, username: r.get(1)?, color: r.get(2)? })
}

fn server_row(r: &Row) -> DbResult<Server> {
    Ok(Server { id: r.get(0)?, name: r.get(1)?, owner_id: r.get(2)?, invite: r.get(3)? })
}

fn channel_row(r: &Row) -> DbResult<Channel> {
    Ok(Channel { id: r.get(0)?, server_id: r.get(1)?, name: r.get(2)?, topic: r.get(3)? })
}

const MSG_SELECT: &str = "SELECT m.id, m.channel_id, m.content, m.created_at, m.edited_at,
                                 u.id, u.username, u.color
                          FROM messages m JOIN users u ON u.id = m.author_id";

fn message_row(r: &Row) -> DbResult<Message> {
    Ok(Message {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        content: r.get(2)?,
        created_at: r.get(3)?,
        edited_at: r.get(4)?,
        author: User { id: r.get(5)?, username: r.get(6)?, color: r.get(7)? },
    })
}

// ---------- пользователи и сессии ----------

pub fn username_taken(c: &Connection, name: &str) -> DbResult<bool> {
    c.query_row("SELECT 1 FROM users WHERE username = ?1", [name], |_| Ok(()))
        .optional()
        .map(|o| o.is_some())
}

pub fn create_user(c: &Connection, username: &str, hash: &str, color: &str) -> DbResult<User> {
    c.execute(
        "INSERT INTO users (username, password_hash, color, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![username, hash, color, now_ms()],
    )?;
    Ok(User { id: c.last_insert_rowid(), username: username.into(), color: color.into() })
}

pub fn user_with_hash(c: &Connection, username: &str) -> DbResult<Option<(User, String)>> {
    c.query_row(
        "SELECT id, username, color, password_hash FROM users WHERE username = ?1",
        [username],
        |r| Ok((user_row(r)?, r.get(3)?)),
    )
    .optional()
}

pub fn create_session(c: &Connection, token: &str, user_id: i64) -> DbResult<()> {
    c.execute(
        "INSERT INTO sessions (token, user_id, created_at) VALUES (?1, ?2, ?3)",
        params![token, user_id, now_ms()],
    )?;
    Ok(())
}

pub fn delete_session(c: &Connection, token: &str) -> DbResult<()> {
    c.execute("DELETE FROM sessions WHERE token = ?1", [token])?;
    Ok(())
}

pub fn user_by_token(c: &Connection, token: &str) -> DbResult<Option<User>> {
    c.query_row(
        "SELECT u.id, u.username, u.color FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token = ?1",
        [token],
        user_row,
    )
    .optional()
}

// ---------- серверы и участники ----------

pub fn servers_for_user(c: &Connection, user_id: i64) -> DbResult<Vec<Server>> {
    let mut st = c.prepare(
        "SELECT s.id, s.name, s.owner_id, s.invite FROM servers s
         JOIN members m ON m.server_id = s.id
         WHERE m.user_id = ?1 ORDER BY m.joined_at, s.id",
    )?;
    let rows = st.query_map([user_id], server_row)?;
    rows.collect()
}

pub fn server(c: &Connection, id: i64) -> DbResult<Option<Server>> {
    c.query_row("SELECT id, name, owner_id, invite FROM servers WHERE id = ?1", [id], server_row)
        .optional()
}

pub fn server_by_invite(c: &Connection, invite: &str) -> DbResult<Option<Server>> {
    c.query_row(
        "SELECT id, name, owner_id, invite FROM servers WHERE invite = ?1",
        [invite],
        server_row,
    )
    .optional()
}

/// Создаёт сервер, делает владельца участником и добавляет канал «общий».
pub fn create_server(c: &mut Connection, name: &str, owner: i64, invite: &str) -> DbResult<Server> {
    let tx = c.transaction()?;
    let t = now_ms();
    tx.execute(
        "INSERT INTO servers (name, owner_id, invite, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![name, owner, invite, t],
    )?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO members (server_id, user_id, joined_at) VALUES (?1, ?2, ?3)",
        params![id, owner, t],
    )?;
    tx.execute(
        "INSERT INTO channels (server_id, name, topic, created_at) VALUES (?1, 'общий', '', ?2)",
        params![id, t],
    )?;
    tx.commit()?;
    Ok(Server { id, name: name.into(), owner_id: owner, invite: invite.into() })
}

/// Возвращает true, если пользователь был добавлен (а не состоял уже).
pub fn add_member(c: &Connection, server_id: i64, user_id: i64) -> DbResult<bool> {
    let n = c.execute(
        "INSERT OR IGNORE INTO members (server_id, user_id, joined_at) VALUES (?1, ?2, ?3)",
        params![server_id, user_id, now_ms()],
    )?;
    Ok(n > 0)
}

pub fn is_member(c: &Connection, server_id: i64, user_id: i64) -> DbResult<bool> {
    c.query_row(
        "SELECT 1 FROM members WHERE server_id = ?1 AND user_id = ?2",
        [server_id, user_id],
        |_| Ok(()),
    )
    .optional()
    .map(|o| o.is_some())
}

pub fn shares_server(c: &Connection, a: i64, b: i64) -> DbResult<bool> {
    c.query_row(
        "SELECT 1 FROM members x JOIN members y ON x.server_id = y.server_id
         WHERE x.user_id = ?1 AND y.user_id = ?2 LIMIT 1",
        [a, b],
        |_| Ok(()),
    )
    .optional()
    .map(|o| o.is_some())
}

pub fn members(c: &Connection, server_id: i64) -> DbResult<Vec<User>> {
    let mut st = c.prepare(
        "SELECT u.id, u.username, u.color FROM members m JOIN users u ON u.id = m.user_id
         WHERE m.server_id = ?1 ORDER BY u.username COLLATE NOCASE",
    )?;
    let rows = st.query_map([server_id], user_row)?;
    rows.collect()
}

// ---------- каналы ----------

pub fn channels(c: &Connection, server_id: i64) -> DbResult<Vec<Channel>> {
    let mut st = c.prepare(
        "SELECT id, server_id, name, topic FROM channels WHERE server_id = ?1 ORDER BY id",
    )?;
    let rows = st.query_map([server_id], channel_row)?;
    rows.collect()
}

pub fn channel(c: &Connection, id: i64) -> DbResult<Option<Channel>> {
    c.query_row(
        "SELECT id, server_id, name, topic FROM channels WHERE id = ?1",
        [id],
        channel_row,
    )
    .optional()
}

pub fn create_channel(c: &Connection, server_id: i64, name: &str, topic: &str) -> DbResult<Channel> {
    c.execute(
        "INSERT INTO channels (server_id, name, topic, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![server_id, name, topic, now_ms()],
    )?;
    Ok(Channel { id: c.last_insert_rowid(), server_id, name: name.into(), topic: topic.into() })
}

// ---------- сообщения ----------

/// Последние `limit` сообщений канала с id меньше `before`, по возрастанию.
pub fn messages(c: &Connection, channel_id: i64, before: Option<i64>, limit: i64) -> DbResult<Vec<Message>> {
    let sql = format!("{MSG_SELECT} WHERE m.channel_id = ?1 AND m.id < ?2 ORDER BY m.id DESC LIMIT ?3");
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params![channel_id, before.unwrap_or(i64::MAX), limit], message_row)?;
    let mut out: Vec<Message> = rows.collect::<DbResult<_>>()?;
    out.reverse();
    Ok(out)
}

pub fn message(c: &Connection, id: i64) -> DbResult<Option<Message>> {
    let sql = format!("{MSG_SELECT} WHERE m.id = ?1");
    c.query_row(&sql, [id], message_row).optional()
}

pub fn create_message(c: &Connection, channel_id: i64, author_id: i64, content: &str) -> DbResult<Message> {
    c.execute(
        "INSERT INTO messages (channel_id, author_id, content, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![channel_id, author_id, content, now_ms()],
    )?;
    let id = c.last_insert_rowid();
    message(c, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn update_message(c: &Connection, id: i64, content: &str) -> DbResult<()> {
    c.execute(
        "UPDATE messages SET content = ?1, edited_at = ?2 WHERE id = ?3",
        params![content, now_ms(), id],
    )?;
    Ok(())
}

pub fn delete_message(c: &Connection, id: i64) -> DbResult<()> {
    c.execute("DELETE FROM messages WHERE id = ?1", [id])?;
    Ok(())
}
