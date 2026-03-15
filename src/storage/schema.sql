CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY,
  started_at TEXT NOT NULL,
  provider TEXT NOT NULL,
  model TEXT,
  client_hint TEXT
);

CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id),
  direction TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  role TEXT,
  content TEXT NOT NULL,
  raw_http BLOB,
  tokens_in INTEGER,
  tokens_out INTEGER
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id, timestamp);
