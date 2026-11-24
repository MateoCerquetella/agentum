//! The one SQLite file (data/cerq.db). Tables keep the column names the
//! Odysseus/cerq-mail data used, so the import is a straight copy.
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS users (name TEXT PRIMARY KEY, password_hash TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS auth_sessions (token TEXT PRIMARY KEY, user TEXT NOT NULL, expiry INTEGER NOT NULL);

-- Mail accounts (secrets are "enc:" Fernet values)
CREATE TABLE IF NOT EXISTS email_accounts (
  id TEXT PRIMARY KEY, name TEXT, is_default INTEGER DEFAULT 0, enabled INTEGER DEFAULT 1,
  imap_host TEXT, imap_port INTEGER DEFAULT 993, imap_user TEXT, imap_password TEXT, imap_starttls INTEGER DEFAULT 0,
  smtp_host TEXT, smtp_port INTEGER DEFAULT 465, smtp_security TEXT DEFAULT 'ssl', smtp_user TEXT, smtp_password TEXT,
  from_address TEXT, display_name TEXT, oauth_provider TEXT, oauth_access_token TEXT, oauth_refresh_token TEXT,
  oauth_token_expiry TEXT, created_at TEXT, updated_at TEXT);
-- Mail cache (same as cerq-mail cache.db)
CREATE TABLE IF NOT EXISTS msg (account TEXT, folder TEXT, uid INTEGER, uidvalidity INTEGER, msgid TEXT, in_reply_to TEXT, refs TEXT, thrid TEXT,
  from_name TEXT, from_addr TEXT, to_addrs TEXT, cc_addrs TEXT, subject TEXT, ts INTEGER, flags TEXT, labels TEXT,
  list_unsub TEXT, list_unsub_post TEXT, list_id TEXT, precedence TEXT, auto_submitted TEXT, ctype TEXT,
  category TEXT, ai INTEGER DEFAULT 0, extra TEXT, PRIMARY KEY (account, folder, uid));
CREATE INDEX IF NOT EXISTS msg_ts ON msg(account, folder, ts);
CREATE INDEX IF NOT EXISTS msg_from ON msg(from_addr);
CREATE INDEX IF NOT EXISTS msg_id ON msg(msgid);
CREATE TABLE IF NOT EXISTS actlog (id TEXT PRIMARY KEY, ts INTEGER, account TEXT, rule TEXT, action TEXT, msgid TEXT, subject TEXT,
  from_addr TEXT, src TEXT, dst TEXT, status TEXT, detail TEXT, undone INTEGER DEFAULT 0);
CREATE TABLE IF NOT EXISTS folders (account TEXT PRIMARY KEY, data TEXT);
-- Opened and prefetched messages, as GET /api/mail/read answers them (JSON)
CREATE TABLE IF NOT EXISTS mail_body (account TEXT, folder TEXT, uid INTEGER, ts INTEGER, data TEXT, PRIMARY KEY (account, folder, uid));
CREATE TABLE IF NOT EXISTS uploads (token TEXT PRIMARY KEY, filename TEXT, ctype TEXT, size INTEGER, path TEXT, ts INTEGER);

-- Calendars
CREATE TABLE IF NOT EXISTS calendar_accounts (id TEXT PRIMARY KEY, label TEXT, kind TEXT, url TEXT, username TEXT, password TEXT, created_at TEXT);
CREATE TABLE IF NOT EXISTS calendars (id TEXT PRIMARY KEY, account_id TEXT, name TEXT, color TEXT, source TEXT DEFAULT 'local',
  remote_url TEXT, read_only INTEGER DEFAULT 0, ctag TEXT, created_at TEXT, updated_at TEXT);
CREATE TABLE IF NOT EXISTS calendar_events (uid TEXT PRIMARY KEY, calendar_id TEXT, summary TEXT, description TEXT, location TEXT,
  dtstart TEXT, dtend TEXT, all_day INTEGER DEFAULT 0, is_utc INTEGER DEFAULT 1, rrule TEXT, recurrence_exdates TEXT,
  color TEXT, status TEXT, origin TEXT, remote_href TEXT, remote_etag TEXT, sync_pending TEXT, created_at TEXT, updated_at TEXT);
CREATE INDEX IF NOT EXISTS ev_cal ON calendar_events(calendar_id);
CREATE INDEX IF NOT EXISTS ev_start ON calendar_events(dtstart);
CREATE TABLE IF NOT EXISTS calendar_tombstones (uid TEXT PRIMARY KEY, calendar_id TEXT, remote_href TEXT, remote_etag TEXT, ts INTEGER);
CREATE TABLE IF NOT EXISTS calendar_done (key TEXT PRIMARY KEY, ts INTEGER);
CREATE TABLE IF NOT EXISTS calendar_people (uid TEXT PRIMARY KEY, people TEXT);

-- Notes and to-dos
CREATE TABLE IF NOT EXISTS notes (id TEXT PRIMARY KEY, title TEXT, content TEXT, items TEXT, note_type TEXT DEFAULT 'note',
  color TEXT, label TEXT, pinned INTEGER DEFAULT 0, archived INTEGER DEFAULT 0, due_date TEXT, sort_order INTEGER DEFAULT 0,
  created_at TEXT, updated_at TEXT);

-- Chat
CREATE TABLE IF NOT EXISTS chat_sessions (id TEXT PRIMARY KEY, name TEXT, archived INTEGER DEFAULT 0, pinned INTEGER DEFAULT 0,
  created_at TEXT, updated_at TEXT);
CREATE TABLE IF NOT EXISTS chat_messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT REFERENCES chat_sessions(id) ON DELETE CASCADE,
  role TEXT, content TEXT, meta TEXT, ts TEXT);
CREATE INDEX IF NOT EXISTS chat_msg_s ON chat_messages(session_id);
CREATE TABLE IF NOT EXISTS memories (id TEXT PRIMARY KEY, text TEXT, category TEXT, created_at TEXT);
-- Files attached in chat: the text the page extracted (PDF, Word, Excel…), read by the model in parts.
CREATE TABLE IF NOT EXISTS chat_files (id TEXT PRIMARY KEY, name TEXT, kind TEXT, size INTEGER, text TEXT, created_at TEXT);
"#;

pub fn migrate(c: &rusqlite::Connection) -> rusqlite::Result<()> { c.execute_batch(SCHEMA) }
