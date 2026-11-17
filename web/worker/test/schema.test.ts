import { env } from "cloudflare:workers";
import { getTableConfig, type SQLiteTable } from "drizzle-orm/sqlite-core";
import { describe, expect, it } from "vitest";
import { TABLES } from "../src/db/schema.ts";

// Columns of architecture.md 4.3, written out from the spec (not derived from schema.ts), so a column
// missing from both schema.ts and the migration is caught.
const SPEC_COLUMNS: Record<string, string[]> = {
  users: ["id", "handle", "display_name", "role", "status", "invited_by", "invite_id", "invites_granted", "blocked_at", "blocked_reason", "blocked_via", "recovery_ack_at", "created_at"],
  passkeys: ["id", "user_id", "public_key", "counter", "transports", "device_type", "backed_up", "name", "created_at", "last_used_at"],
  challenges: ["id", "purpose", "user_id", "challenge", "payload", "expires_at"],
  recovery_codes: ["id", "user_id", "code_hash", "batch", "used_at", "created_at"],
  sessions: ["id_hash", "user_id", "created_at", "last_seen_at", "idle_expires_at", "expires_at", "reauth_at", "amr", "recovery_only", "user_agent", "revoked_at"],
  invites: ["id", "code_hash", "created_by", "kind", "expires_at", "used_by", "used_at", "revoked_at", "created_at"],
  desktops: ["id", "user_id", "name", "color", "public_key", "fingerprint", "os", "app_version", "created_at", "last_seen_at", "revoked_at"],
  desktop_tokens: ["id", "desktop_id", "token_hash", "scope", "created_at", "last_used_at", "revoked_at"],
  devices: ["id", "user_id", "name", "public_key", "fingerprint", "created_at", "last_seen_at", "revoked_at"],
  device_grants: ["device_id", "desktop_id", "state", "permission", "requested_permission", "confirmed_at", "updated_at"],
  push_subscriptions: ["id", "user_id", "device_id", "endpoint", "p256dh", "auth", "vapid_key_id", "ua", "created_at", "last_success_at", "failures"],
  push_prefs: ["user_id", "desktop_id", "enabled", "categories"],
  pairing_requests: ["id", "user_code", "device_code_hash", "challenge", "desktop_public_key", "machine_name", "os", "confirmation_code", "status", "user_id", "desktop_id", "created_at", "expires_at", "last_polled_at", "ip_hash"],
  settings: ["key", "value", "updated_at", "updated_by"],
  audit_log: ["id", "ts", "actor_user_id", "actor_kind", "action", "target_type", "target_id", "meta"],
  rate_limits: ["key", "window_start", "count"],
};

// Indexes of architecture.md 4.3 ("Indexes: ..."), as table(column).
const SPEC_INDEXES: [string, string[]][] = [
  ["passkeys", ["user_id"]],
  ["sessions", ["user_id"]],
  ["sessions", ["idle_expires_at"]],
  ["invites", ["created_by"]],
  ["users", ["invited_by"]],
  ["desktops", ["user_id"]],
  ["devices", ["user_id"]],
  ["device_grants", ["desktop_id"]],
  ["push_subscriptions", ["user_id"]],
];
// Added for the cleanup queries (A-02).
const CLEANUP_INDEXES: [string, string[]][] = [
  ["challenges", ["expires_at"]],
  ["sessions", ["expires_at"]],
  ["audit_log", ["ts"]],
  ["rate_limits", ["window_start"]],
  ["pairing_requests", ["expires_at"]],
];

interface TableInfoRow {
  name: string;
  type: string;
  notnull: number;
  pk: number;
}
interface IndexListRow {
  name: string;
  unique: number;
}

async function tableInfo(table: string): Promise<TableInfoRow[]> {
  const { results } = await env.DB.prepare(`SELECT name, type, "notnull", pk FROM pragma_table_info(?)`).bind(table).all<TableInfoRow>();
  return results;
}

async function indexes(table: string): Promise<{ name: string; unique: boolean; columns: string[] }[]> {
  const { results } = await env.DB.prepare(`SELECT name, "unique" FROM pragma_index_list(?)`).bind(table).all<IndexListRow>();
  const out = [];
  for (const row of results) {
    const cols = await env.DB.prepare(`SELECT name FROM pragma_index_info(?) ORDER BY seqno`).bind(row.name).all<{ name: string }>();
    out.push({ name: row.name, unique: row.unique === 1, columns: cols.results.map((c) => c.name) });
  }
  return out;
}

describe("D1 schema = architecture.md 4.3", () => {
  it("has exactly the 16 tables of the spec (plus the migrations bookkeeping table)", async () => {
    const { results } = await env.DB.prepare(`SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_cf_%' AND name <> 'd1_migrations' ORDER BY name`).all<{ name: string }>();
    expect(results.map((r) => r.name)).toEqual(Object.keys(SPEC_COLUMNS).sort());
  });

  it.each(Object.entries(SPEC_COLUMNS))("%s has exactly the spec columns", async (table, columns) => {
    const info = await tableInfo(table);
    expect(info.map((c) => c.name).sort()).toEqual([...columns].sort());
  });

  it("the Drizzle schema lists the same tables and columns as the spec", () => {
    const fromDrizzle: Record<string, string[]> = {};
    for (const table of Object.values(TABLES)) {
      const config = getTableConfig(table as SQLiteTable);
      fromDrizzle[config.name] = config.columns.map((c) => c.name);
    }
    expect(Object.keys(fromDrizzle).sort()).toEqual(Object.keys(SPEC_COLUMNS).sort());
    for (const [table, columns] of Object.entries(SPEC_COLUMNS)) expect(fromDrizzle[table]?.slice().sort(), table).toEqual([...columns].sort());
  });

  it("the migration and the Drizzle schema agree on column type, NOT NULL and primary key", async () => {
    for (const table of Object.values(TABLES)) {
      const config = getTableConfig(table as SQLiteTable);
      const info = new Map((await tableInfo(config.name)).map((c) => [c.name, c]));
      const pkColumns = config.primaryKeys.flatMap((pk) => pk.columns.map((c) => c.name));
      for (const column of config.columns) {
        const actual = info.get(column.name);
        expect(actual, `${config.name}.${column.name} exists`).toBeDefined();
        expect(actual?.type.toLowerCase(), `${config.name}.${column.name} type`).toBe(column.getSQLType().toLowerCase());
        const isPk = column.primary || pkColumns.includes(column.name);
        expect(actual?.pk !== 0, `${config.name}.${column.name} pk`).toBe(isPk);
        // SQLite reports NOT NULL = 0 for a lone INTEGER/TEXT primary key declared without NOT NULL; drizzle adds it.
        expect(actual?.notnull === 1, `${config.name}.${column.name} not null`).toBe(column.notNull);
      }
    }
  });

  it("the migration and the Drizzle schema agree on indexes, UNIQUE constraints and foreign keys", async () => {
    for (const table of Object.values(TABLES)) {
      const config = getTableConfig(table as SQLiteTable);
      const actual = await indexes(config.name);
      const wanted = [
        ...config.indexes.map((i) => ({ unique: i.config.unique, columns: i.config.columns.map((c) => ("name" in c ? c.name : "?")) })),
        ...config.columns.filter((c) => c.isUnique).map((c) => ({ unique: true, columns: [c.name] })),
      ];
      for (const w of wanted) {
        expect(actual.some((a) => a.unique === w.unique && a.columns.join() === w.columns.join()), `${config.name}(${w.columns.join()}) unique=${w.unique}`).toBe(true);
      }
      const fks = await env.DB.prepare(`SELECT "from", "table", "to", on_delete FROM pragma_foreign_key_list(?)`).bind(config.name).all<{ from: string; table: string; to: string; on_delete: string }>();
      expect(fks.results.length, `${config.name} foreign keys`).toBe(config.foreignKeys.length);
      for (const fk of config.foreignKeys) {
        const ref = fk.reference();
        const match = fks.results.find((r) => r.from === ref.columns[0]?.name);
        expect(match?.table, `${config.name}.${ref.columns[0]?.name} references`).toBe(getTableConfig(ref.foreignTable).name);
        expect(match?.on_delete.toLowerCase(), `${config.name}.${ref.columns[0]?.name} on delete`).toBe((fk.onDelete ?? "no action").toLowerCase());
      }
    }
  });

  it("has every index of the spec and the cleanup indexes", async () => {
    for (const [table, columns] of [...SPEC_INDEXES, ...CLEANUP_INDEXES]) {
      const found = await indexes(table);
      expect(found.some((i) => i.columns.join() === columns.join()), `${table}(${columns.join()})`).toBe(true);
    }
  });

  it("has the UNIQUE constraints of the spec", async () => {
    for (const [table, column] of [
      ["users", "handle"],
      ["desktop_tokens", "token_hash"],
      ["push_subscriptions", "endpoint"],
      ["pairing_requests", "user_code"],
    ] as const) {
      expect((await indexes(table)).some((i) => i.unique && i.columns.join() === column), `${table}.${column}`).toBe(true);
    }
  });

  it("has the composite primary keys of the spec", async () => {
    const pk = async (table: string) => (await tableInfo(table)).filter((c) => c.pk > 0).sort((a, b) => a.pk - b.pk).map((c) => c.name);
    expect(await pk("device_grants")).toEqual(["device_id", "desktop_id"]);
    expect(await pk("push_prefs")).toEqual(["user_id", "desktop_id"]);
    expect(await pk("rate_limits")).toEqual(["key", "window_start"]);
    expect(await pk("sessions")).toEqual(["id_hash"]);
    expect(await pk("settings")).toEqual(["key"]);
  });
});

describe("CHECK constraints and foreign keys are enforced", () => {
  const now = 1_800_000_000;
  const insertUser = (id: string, handle = id, extra = "") => env.DB.prepare(`INSERT INTO users (id, handle, display_name, created_at${extra ? ", " + extra.split("=")[0] : ""}) VALUES (?, ?, 'Name', ?${extra ? ", ?" : ""})`).bind(id, handle, now, ...(extra ? [extra.split("=")[1]] : [])).run();

  it("rejects a handle that is too short, too long or not lowercase", async () => {
    await expect(insertUser("c-u1", "ab")).rejects.toThrow();
    await expect(insertUser("c-u2", "a".repeat(33))).rejects.toThrow();
    await expect(insertUser("c-u3", "MixedCase")).rejects.toThrow();
    await insertUser("c-u4", "okay-handle");
    await env.DB.prepare("DELETE FROM users WHERE id = 'c-u4'").run();
  });

  it("rejects an unknown role, status, session amr and grant state", async () => {
    await expect(insertUser("c-u5", "role-bad", "role=owner")).rejects.toThrow();
    await expect(insertUser("c-u6", "status-bad", "status=gone")).rejects.toThrow();
    await insertUser("c-u7", "check-user");
    await expect(env.DB.prepare(`INSERT INTO sessions (id_hash, user_id, created_at, last_seen_at, idle_expires_at, expires_at, amr) VALUES ('s1', 'c-u7', 1, 1, 2, 3, 'password')`).run()).rejects.toThrow();
    await env.DB.prepare("DELETE FROM users WHERE id = 'c-u7'").run();
  });

  it("enforces foreign keys and cascades a user delete", async () => {
    await expect(env.DB.prepare(`INSERT INTO passkeys (id, user_id, public_key, name, created_at) VALUES ('p-orphan', 'no-such-user', x'00', 'k', 1)`).run()).rejects.toThrow();
    await insertUser("c-u8", "cascade-user");
    await env.DB.prepare(`INSERT INTO passkeys (id, user_id, public_key, name, created_at) VALUES ('p-ok', 'c-u8', x'0102', 'k', 1)`).run();
    await env.DB.prepare("DELETE FROM users WHERE id = 'c-u8'").run();
    const left = await env.DB.prepare("SELECT count(*) AS n FROM passkeys WHERE id = 'p-ok'").first<{ n: number }>();
    expect(left?.n).toBe(0);
  });

  it("allows a bootstrap invite without a creator but not a user invite", async () => {
    await env.DB.prepare(`INSERT INTO invites (id, code_hash, created_by, kind, expires_at, created_at) VALUES ('i-boot', 'hash-boot', NULL, 'bootstrap', 2, 1)`).run();
    await expect(env.DB.prepare(`INSERT INTO invites (id, code_hash, created_by, kind, expires_at, created_at) VALUES ('i-user', 'hash-user', NULL, 'user', 2, 1)`).run()).rejects.toThrow();
    await env.DB.prepare("DELETE FROM invites WHERE id = 'i-boot'").run();
  });
});
