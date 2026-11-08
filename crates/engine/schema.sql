-- base schema. Single source of truth, applied on every boot by both stores (native open, Worker cold start).
--
-- Rules:
--  * Every statement must be idempotent (IF NOT EXISTS / INSERT OR IGNORE). Additive changes go at the end.
--  * Statements are separated by a line holding only `--;;` (triggers contain `;`, and D1 cannot exec a script).
--  * The whole file runs as one atomic batch, so concurrent boots serialize and a failure leaves nothing half-applied.

CREATE TABLE IF NOT EXISTS docs (
  db      TEXT    NOT NULL,
  id      TEXT    NOT NULL,
  rev     INTEGER NOT NULL,
  seq     INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  body    TEXT    NOT NULL,
  PRIMARY KEY (db, id)
) WITHOUT ROWID;
--;;
CREATE INDEX IF NOT EXISTS docs_seq ON docs (db, seq);
--;;
CREATE TABLE IF NOT EXISTS dbs (
  name        TEXT PRIMARY KEY,
  created_seq INTEGER NOT NULL
) WITHOUT ROWID;
--;;
CREATE TABLE IF NOT EXISTS meta (
  k TEXT PRIMARY KEY,
  v INTEGER NOT NULL
) WITHOUT ROWID;
--;;
INSERT OR IGNORE INTO meta (k, v) VALUES ('seq', 0);
--;;
CREATE TABLE IF NOT EXISTS indexes (
  db     TEXT NOT NULL,
  name   TEXT NOT NULL,
  fields TEXT NOT NULL,
  PRIMARY KEY (db, name)
) WITHOUT ROWID;
--;;
-- Commit guards. A guard statement inserts 0 when its condition fails; the CHECK
-- aborts the whole batch, which rolls back. Rows are deleted at the end of each commit.
CREATE TABLE IF NOT EXISTS _guard (
  ok INTEGER NOT NULL,
  CONSTRAINT base_conflict CHECK (ok = 1)
);
--;;
-- O(1) live-doc counts per db, kept by triggers. delete_db removes its row explicitly;
-- there is no DELETE trigger so bulk deletes stay cheap.
CREATE TABLE IF NOT EXISTS db_counts (
  db TEXT PRIMARY KEY,
  n  INTEGER NOT NULL
) WITHOUT ROWID;
--;;
-- One-time backfill for databases created before db_counts existed. The uncorrelated NOT EXISTS
-- is evaluated once, so later boots skip the scan. Runs in the same atomic batch as the triggers
-- below, so no write can slip between the backfill and the triggers.
INSERT OR IGNORE INTO db_counts (db, n)
  SELECT db, COUNT(*) FROM docs WHERE deleted = 0 AND NOT EXISTS (SELECT 1 FROM db_counts) GROUP BY db;
--;;
CREATE TRIGGER IF NOT EXISTS docs_count_ins AFTER INSERT ON docs WHEN NEW.deleted = 0
BEGIN
  INSERT INTO db_counts (db, n) VALUES (NEW.db, 1) ON CONFLICT (db) DO UPDATE SET n = n + 1;
END;
--;;
CREATE TRIGGER IF NOT EXISTS docs_count_revive AFTER UPDATE OF deleted ON docs WHEN OLD.deleted = 1 AND NEW.deleted = 0
BEGIN
  INSERT INTO db_counts (db, n) VALUES (NEW.db, 1) ON CONFLICT (db) DO UPDATE SET n = n + 1;
END;
--;;
CREATE TRIGGER IF NOT EXISTS docs_count_tombstone AFTER UPDATE OF deleted ON docs WHEN OLD.deleted = 0 AND NEW.deleted = 1
BEGIN
  UPDATE db_counts SET n = n - 1 WHERE db = NEW.db;
END;
