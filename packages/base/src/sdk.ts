import { createBase, type Base } from "./index.js";
import type { SqlValue, Store } from "./types.js";

/** Error from the engine. `status` and `error` follow the HTTP API (409 `conflict`, 404 `not_found`, ...). */
export class BaseError extends Error {
  constructor(
    readonly status: number,
    readonly error: string,
    readonly reason: string,
  ) {
    super(`${error}: ${reason}`);
    this.name = "BaseError";
  }
  get isConflict(): boolean {
    return this.status === 409;
  }
}

export type Doc<T = Record<string, unknown>> = T & { _id: string; _rev: string };
export type Selector = Record<string, unknown>;

export interface FindQuery {
  selector: Selector;
  /** `["age", { name: "desc" }]` */
  sort?: (string | Record<string, "asc" | "desc">)[];
  limit?: number;
  skip?: number;
}
export interface FindResult<T> {
  docs: Doc<T>[];
  /** Use as a `db` guard in `txn` to protect the result from phantoms. */
  update_seq: number;
  warning?: string;
}

export interface AllDocsOptions {
  startkey?: string;
  endkey?: string;
  limit?: number;
  skip?: number;
  descending?: boolean;
  includeDocs?: boolean;
}
export interface AllDocsResult<T> {
  offset: number;
  rows: { id: string; key: string; value: { rev: string }; doc?: Doc<T> }[];
  update_seq: number;
}

export interface ChangesOptions {
  since?: number;
  limit?: number;
  includeDocs?: boolean;
}
export interface ChangesResult<T> {
  results: { seq: number; id: string; changes: { rev: string }[]; deleted?: boolean; doc?: Doc<T> }[];
  last_seq: number;
}

export interface DbInfo {
  db_name: string;
  doc_count: number;
  update_seq: number;
}
export interface Written {
  id: string;
  rev: string;
}

export type TxnOp =
  | { op: "put"; db: string; id: string; doc: Record<string, unknown>; rev?: string }
  | { op: "delete"; db: string; id: string; rev: string };
export interface TxnInput {
  /** Docs you depended on. No `rev` = must be absent. */
  reads?: { db: string; id: string; rev?: string }[];
  /** Fail if anything in `db` changed after `seq`. */
  guards?: { kind: "db"; db: string; seq: number }[];
  ops: TxnOp[];
}
export interface TxnResult {
  seq: number;
  results: { db: string; id: string; rev: string }[];
}

type Call = (method: string, path: string, query?: Record<string, string | number | boolean | undefined>, body?: unknown) => Promise<any>;

const enc = encodeURIComponent;

/** One database (like a collection). Get one with `base.db(name)`. */
export class Collection<T extends object = Record<string, unknown>> {
  constructor(
    readonly name: string,
    private readonly call: Call,
  ) {}

  private get p() {
    return `/${enc(this.name)}`;
  }

  /** The doc, or `null` if missing or deleted. */
  async get(id: string): Promise<Doc<T> | null> {
    try {
      return await this.call("GET", `${this.p}/${enc(id)}`);
    } catch (e) {
      if (e instanceof BaseError && e.status === 404 && e.error === "not_found") return null;
      throw e;
    }
  }

  /** Create (no `rev`, throws a conflict if it exists) or update (`rev` = the revision you read). */
  put(id: string, doc: T, opts: { rev?: string } = {}): Promise<Written> {
    return this.call("PUT", `${this.p}/${enc(id)}`, { rev: opts.rev }, doc);
  }

  /** Create with a generated id (or `_id` in the doc). */
  post(doc: T): Promise<Written> {
    return this.call("POST", this.p, {}, doc);
  }

  delete(id: string, rev: string): Promise<Written> {
    return this.call("DELETE", `${this.p}/${enc(id)}`, { rev });
  }

  /**
   * Read-modify-write with retry on conflict. `fn` gets the current doc (or `null`) and returns the new body.
   * Return `undefined` to leave the doc unchanged.
   */
  async update(id: string, fn: (doc: Doc<T> | null) => T | undefined | Promise<T | undefined>, retries = 5): Promise<Written | undefined> {
    for (let i = 0; ; i++) {
      const cur = await this.get(id);
      const next = await fn(cur);
      if (next === undefined) return cur ? { id, rev: cur._rev } : undefined;
      try {
        return await this.put(id, next, { rev: cur?._rev });
      } catch (e) {
        if (!(e instanceof BaseError && e.isConflict) || i >= retries) throw e;
      }
    }
  }

  find(q: FindQuery): Promise<FindResult<T>> {
    return this.call("POST", `${this.p}/_find`, {}, q);
  }

  allDocs(o: AllDocsOptions = {}): Promise<AllDocsResult<T>> {
    return this.call("GET", `${this.p}/_all_docs`, {
      startkey: o.startkey,
      endkey: o.endkey,
      limit: o.limit,
      skip: o.skip,
      descending: o.descending,
      include_docs: o.includeDocs,
    });
  }

  /** Latest state per doc since `since`. Poll with `since = last_seq`. */
  changes(o: ChangesOptions = {}): Promise<ChangesResult<T>> {
    return this.call("GET", `${this.p}/_changes`, { since: o.since, limit: o.limit, include_docs: o.includeDocs });
  }

  info(): Promise<DbInfo> {
    return this.call("GET", this.p);
  }

  async createIndex(fields: string[], name?: string): Promise<{ result: "created" | "exists"; name: string }> {
    return this.call("POST", `${this.p}/_index`, {}, { fields, name });
  }
  async indexes(): Promise<{ name: string; fields: string[] }[]> {
    return (await this.call("GET", `${this.p}/_index`)).indexes;
  }
  async dropIndex(name: string): Promise<void> {
    await this.call("DELETE", `${this.p}/_index/${enc(name)}`);
  }
}

export interface Sdk {
  /** Handle for an existing database. Does not create it. */
  db<T extends object = Record<string, unknown>>(name: string): Collection<T>;
  /** Create the database; returns its handle. If it exists already, returns the handle too. */
  createDb<T extends object = Record<string, unknown>>(name: string): Promise<Collection<T>>;
  dropDb(name: string): Promise<void>;
  listDbs(): Promise<string[]>;
  /** Atomic multi-doc, multi-db transaction. Throws `BaseError` with `isConflict` on stale revs or guards. */
  txn(input: TxnInput): Promise<TxnResult>;
  /** The underlying Request/Response handler, if you also want to expose the HTTP API. Auth is off: do not expose it as is. */
  readonly raw: Base;
}

export interface OpenOptions {
  store: Store;
  wasm?: Parameters<typeof createBase>[0]["wasm"];
}

/**
 * Open a database in-process: TS -> SDK -> engine (wasm) -> your SQLite. No HTTP, no keys.
 * The SDK is fully trusted (admin on everything).
 */
export async function openBase(opts: OpenOptions): Promise<Sdk> {
  const raw = await createBase({ store: opts.store, wasm: opts.wasm, noAuth: true });
  const dec = new TextDecoder();
  const enc8 = new TextEncoder();

  const call: Call = async (method, path, query, body) => {
    const qs = new URLSearchParams();
    for (const [k, v] of Object.entries(query ?? {})) if (v !== undefined) qs.set(k, String(v));
    const res = await raw.handle({
      method,
      path,
      query: qs.toString(),
      body: body === undefined ? undefined : enc8.encode(JSON.stringify(body)),
    });
    const json = res.body.length ? JSON.parse(dec.decode(res.body)) : undefined;
    if (res.status >= 400) throw new BaseError(res.status, json?.error ?? "error", json?.reason ?? "");
    return json;
  };

  const sdk: Sdk = {
    raw,
    db: <T extends object>(name: string) => new Collection<T>(name, call),
    async createDb<T extends object>(name: string) {
      try {
        await call("PUT", `/${enc(name)}`);
      } catch (e) {
        if (!(e instanceof BaseError && e.status === 412)) throw e;
      }
      return new Collection<T>(name, call);
    },
    async dropDb(name) {
      await call("DELETE", `/${enc(name)}`);
    },
    listDbs: () => call("GET", "/_all_dbs"),
    txn: (input) => call("POST", "/_txn", {}, input),
  };
  return sdk;
}

export type { SqlValue };
