import type { SqlValue, Store } from "../types.js";

/** Synchronous SQLite handle: `node:sqlite` DatabaseSync, `better-sqlite3`, or `bun:sqlite` Database. */
export interface SyncSqlite {
  exec(sql: string): unknown;
  prepare(sql: string): {
    all(...params: any[]): unknown[];
    run(...params: any[]): unknown;
    /** better-sqlite3: false for statements that return no data. */
    reader?: boolean;
  };
}

/**
 * Store over a synchronous SQLite handle. Each batch is one `BEGIN IMMEDIATE` transaction.
 * Everything runs synchronously, so batches never interleave inside one process.
 */
export function sqliteStore(db: SyncSqlite): Store {
  return {
    async batch(stmts) {
      db.exec("BEGIN IMMEDIATE");
      try {
        const out: SqlValue[][][] = [];
        for (const s of stmts) {
          const st = db.prepare(s.sql);
          if (st.reader === false) {
            st.run(...s.params);
            out.push([]);
          } else {
            const rows = st.all(...s.params) as (Record<string, SqlValue> | SqlValue[])[];
            out.push(rows.map((r) => (Array.isArray(r) ? r : Object.values(r))));
          }
        }
        db.exec("COMMIT");
        return out;
      } catch (e) {
        try {
          db.exec("ROLLBACK");
        } catch {}
        throw e;
      }
    },
  };
}
