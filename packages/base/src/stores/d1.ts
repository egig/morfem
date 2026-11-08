import type { SqlValue, Store } from "../types.js";

/** Minimal structural type of a Cloudflare D1 binding. */
export interface D1Like {
  prepare(sql: string): { bind(...params: unknown[]): unknown };
  batch(stmts: any[]): Promise<{ results?: any[] }[]>;
}

/** Store over a D1 binding. D1 runs `batch` as one transaction. */
export function d1Store(db: D1Like): Store {
  return {
    async batch(stmts) {
      const prepared = stmts.map((s) => db.prepare(s.sql).bind(...s.params));
      const out = await db.batch(prepared);
      // Rows come back as objects; column order is preserved and SELECT names are unique by contract.
      return out.map((r) => (r.results ?? []).map((row) => Object.values(row) as SqlValue[]));
    },
  };
}
