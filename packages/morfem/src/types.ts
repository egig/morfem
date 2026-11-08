export type SqlValue = string | number | null;

export interface Statement {
  sql: string;
  params: SqlValue[];
}

/**
 * Storage backend. Run `stmts` in order inside ONE atomic transaction (all or nothing) and return,
 * per statement, its rows as arrays of column values (`[]` for statements returning nothing).
 * A thrown error must roll the whole batch back. Its message is inspected for conflicts, so rethrow as is.
 */
export interface Store {
  batch(stmts: Statement[]): Promise<SqlValue[][][]>;
}

export interface ApiKey {
  /** At least 8 characters. */
  key: string;
  access: "admin" | "write" | "read";
  /** Omitted or `["*"]` = all databases. */
  dbs?: string[];
}

export type Auth = { keys: ApiKey[]; noAuth?: false } | { noAuth: true; keys?: undefined };

export type MorfemOptions = Auth & {
  store: Store;
  /**
   * Wasm to load: a `WebAssembly.Module` (Workers), bytes, a `Response`, or a URL.
   * Defaults to the file shipped in this package.
   */
  wasm?: WebAssembly.Module | BufferSource | Response | Promise<Response> | URL | string;
};

export interface RawRequest {
  method: string;
  /** Percent-encoded path, e.g. `/mydb/doc%201`. */
  path: string;
  /** Query string without `?`. */
  query?: string;
  /** Token from `Authorization: Bearer <token>`. */
  bearer?: string | null;
  body?: Uint8Array;
}

export interface RawResponse {
  status: number;
  contentType: string;
  body: Uint8Array;
}
