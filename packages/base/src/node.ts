import { DatabaseSync } from "node:sqlite";
import { openBase, type Sdk } from "./sdk.js";
import { sqliteStore } from "./stores/sqlite.js";

/**
 * Open (or create) a SQLite file and return the SDK. Node >= 22.5. Use `":memory:"` for a throwaway db.
 * WAL and a 5 s busy timeout are set.
 */
export async function openFile(path: string): Promise<Sdk & { close(): void }> {
  const db = new DatabaseSync(path);
  db.exec("PRAGMA busy_timeout=5000");
  if (path !== ":memory:") db.exec("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL");
  const sdk = await openBase({ store: sqliteStore(db) });
  return Object.assign(sdk, { close: () => db.close() });
}

export { openBase, BaseError } from "./sdk.js";
export type * from "./sdk.js";
