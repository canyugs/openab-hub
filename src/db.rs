use rusqlite::{Connection, params};
use std::sync::Mutex;
use serde::{Deserialize, Serialize};

pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    pub channel_id: u64,
    pub author_id: u64,
    pub author_name: String,
    pub is_bot: bool,
    pub content: String,
    pub timestamp: String,
    pub reference_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: u64,
    pub guild_id: u64,
    pub name: String,
    #[serde(rename = "type")]
    pub channel_type: i64,
    pub parent_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bot {
    pub user_id: u64,
    pub username: String,
}

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS guilds (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS channels (
                id INTEGER PRIMARY KEY,
                guild_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                type INTEGER NOT NULL DEFAULT 0,
                parent_id INTEGER
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY,
                channel_id INTEGER NOT NULL,
                author_id INTEGER NOT NULL,
                author_name TEXT NOT NULL,
                is_bot INTEGER NOT NULL DEFAULT 1,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                reference_id INTEGER
            );
            CREATE TABLE IF NOT EXISTS reactions (
                message_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                emoji TEXT NOT NULL,
                PRIMARY KEY (message_id, user_id, emoji)
            );
            CREATE TABLE IF NOT EXISTS bots (
                user_id INTEGER PRIMARY KEY,
                username TEXT NOT NULL,
                token TEXT NOT NULL
            );
        ")?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn ensure_guild(&self, id: u64, name: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO guilds (id, name) VALUES (?1, ?2)",
            params![id as i64, name],
        ).ok();
    }

    pub fn ensure_channel(&self, id: u64, guild_id: u64, name: &str, channel_type: i64) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO channels (id, guild_id, name, type) VALUES (?1, ?2, ?3, ?4)",
            params![id as i64, guild_id as i64, name, channel_type],
        ).ok();
    }

    pub fn register_bot(&self, user_id: u64, username: &str, token: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO bots (user_id, username, token) VALUES (?1, ?2, ?3)",
            params![user_id as i64, username, token],
        ).ok();
    }

    pub fn get_bot_by_token(&self, token: &str) -> Option<Bot> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT user_id, username FROM bots WHERE token = ?1",
            params![token],
            |row| Ok(Bot {
                user_id: row.get::<_, i64>(0)? as u64,
                username: row.get(1)?,
            }),
        ).ok()
    }

    pub fn get_bot(&self, user_id: u64) -> Option<Bot> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT user_id, username FROM bots WHERE user_id = ?1",
            params![user_id as i64],
            |row| Ok(Bot {
                user_id: row.get::<_, i64>(0)? as u64,
                username: row.get(1)?,
            }),
        ).ok()
    }

    pub fn insert_message(&self, msg: &Message) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages (id, channel_id, author_id, author_name, is_bot, content, timestamp, reference_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                msg.id as i64,
                msg.channel_id as i64,
                msg.author_id as i64,
                msg.author_name,
                msg.is_bot as i64,
                msg.content,
                msg.timestamp,
                msg.reference_id.map(|id| id as i64),
            ],
        ).ok();
    }

    pub fn get_message(&self, id: u64) -> Option<Message> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, channel_id, author_id, author_name, is_bot, content, timestamp, reference_id FROM messages WHERE id = ?1",
            params![id as i64],
            |row| Ok(Message {
                id: row.get::<_, i64>(0)? as u64,
                channel_id: row.get::<_, i64>(1)? as u64,
                author_id: row.get::<_, i64>(2)? as u64,
                author_name: row.get(3)?,
                is_bot: row.get::<_, i64>(4)? != 0,
                content: row.get(5)?,
                timestamp: row.get(6)?,
                reference_id: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
            }),
        ).ok()
    }

    pub fn update_message_content(&self, id: u64, content: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET content = ?1 WHERE id = ?2",
            params![content, id as i64],
        ).ok();
    }

    pub fn delete_message(&self, id: u64) {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM messages WHERE id = ?1", params![id as i64]).ok();
    }

    pub fn get_channel_messages(&self, channel_id: u64, limit: u32) -> Vec<Message> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, channel_id, author_id, author_name, is_bot, content, timestamp, reference_id FROM messages WHERE channel_id = ?1 ORDER BY id DESC LIMIT ?2"
        ).unwrap();
        stmt.query_map(params![channel_id as i64, limit], |row| {
            Ok(Message {
                id: row.get::<_, i64>(0)? as u64,
                channel_id: row.get::<_, i64>(1)? as u64,
                author_id: row.get::<_, i64>(2)? as u64,
                author_name: row.get(3)?,
                is_bot: row.get::<_, i64>(4)? != 0,
                content: row.get(5)?,
                timestamp: row.get(6)?,
                reference_id: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
            })
        }).unwrap().filter_map(|r| r.ok()).collect()
    }

    pub fn get_channel(&self, id: u64) -> Option<Channel> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, guild_id, name, type, parent_id FROM channels WHERE id = ?1",
            params![id as i64],
            |row| Ok(Channel {
                id: row.get::<_, i64>(0)? as u64,
                guild_id: row.get::<_, i64>(1)? as u64,
                name: row.get(2)?,
                channel_type: row.get(3)?,
                parent_id: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
            }),
        ).ok()
    }

    pub fn create_channel(&self, id: u64, guild_id: u64, name: &str, channel_type: i64, parent_id: Option<u64>) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO channels (id, guild_id, name, type, parent_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id as i64, guild_id as i64, name, channel_type, parent_id.map(|v| v as i64)],
        ).ok();
    }

    pub fn rename_channel(&self, id: u64, name: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE channels SET name = ?1 WHERE id = ?2",
            params![name, id as i64],
        ).ok();
    }

    pub fn add_reaction(&self, message_id: u64, user_id: u64, emoji: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO reactions (message_id, user_id, emoji) VALUES (?1, ?2, ?3)",
            params![message_id as i64, user_id as i64, emoji],
        ).ok();
    }

    pub fn remove_reaction(&self, message_id: u64, user_id: u64, emoji: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3",
            params![message_id as i64, user_id as i64, emoji],
        ).ok();
    }

    pub fn get_guild_id(&self) -> Option<u64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT id FROM guilds LIMIT 1", [], |row| {
            Ok(row.get::<_, i64>(0)? as u64)
        }).ok()
    }

    pub fn get_threads(&self) -> Vec<Channel> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, guild_id, name, type, parent_id FROM channels WHERE type = 11 ORDER BY id DESC"
        ).unwrap();
        stmt.query_map([], |row| {
            Ok(Channel {
                id: row.get::<_, i64>(0)? as u64,
                guild_id: row.get::<_, i64>(1)? as u64,
                name: row.get(2)?,
                channel_type: row.get(3)?,
                parent_id: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
            })
        }).unwrap().filter_map(|r| r.ok()).collect()
    }
}
