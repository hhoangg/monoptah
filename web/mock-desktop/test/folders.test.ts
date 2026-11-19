import { describe, expect, it } from "vitest";
import { fingerprint } from "../src/index.ts";
import { setup } from "./harness.ts";

type Folders = { id: string; projectId: string; name: string; sessionIds: string[]; collapsed?: boolean }[];

async function folders(client: Awaited<ReturnType<typeof setup>>["client"]): Promise<Folders> {
  return ((await client.call("workspace.list")) as { folders?: Folders }).folders ?? [];
}

describe("session folders", () => {
  it("the catalog lists the desktop's folders with their sessions", async () => {
    const { client } = await setup();
    expect(await folders(client)).toEqual([
      { id: "folder-parser", projectId: "proj-alpha", name: "Parser work", sessionIds: ["sess-local-1", "sess-local-approval"], collapsed: false },
    ]);
  });

  it("a control device creates, renames, moves into, moves out of and deletes a folder, seen by the next poll", async () => {
    const { client } = await setup();
    const created = await client.call("folder.create", { projectId: "proj-alpha", conversationIds: ["sess-local-files"], name: "Reports" });
    const list = await folders(client);
    expect(list.map((f) => f.name)).toEqual(["Reports", "Parser work"]);
    expect(list[0]?.id).toBe(created.folderId);

    await client.call("folder.rename", { projectId: "proj-alpha", folderId: created.folderId, name: "Write-ups" });
    expect((await folders(client))[0]?.name).toBe("Write-ups");

    // Moving the only session out drops the folder; moving into another keeps the order.
    await client.call("folder.move", { projectId: "proj-alpha", conversationId: "sess-local-files", folderId: "folder-parser" });
    let now = await folders(client);
    expect(now.map((f) => f.id)).toEqual(["folder-parser"]);
    expect(now[0]?.sessionIds).toEqual(["sess-local-1", "sess-local-approval", "sess-local-files"]);
    await client.call("folder.move", { projectId: "proj-alpha", conversationId: "sess-local-1", folderId: null });
    now = await folders(client);
    expect(now[0]?.sessionIds).toEqual(["sess-local-approval", "sess-local-files"]);

    await client.call("folder.delete", { projectId: "proj-alpha", folderId: "folder-parser" });
    expect(await folders(client)).toEqual([]);
  });

  it("names unnamed folders like the desktop: New folder, New folder 2", async () => {
    const { client } = await setup();
    await client.call("folder.create", { projectId: "proj-alpha", conversationIds: ["sess-local-files"] });
    await client.call("folder.create", { projectId: "proj-alpha", conversationIds: ["sess-codespace-1"] });
    expect((await folders(client)).map((f) => f.name)).toEqual(["New folder 2", "New folder", "Parser work"]);
  });

  it("refuses unknown folders and sessions of another project", async () => {
    const { client } = await setup();
    expect(await client.request("folder.rename", { projectId: "proj-alpha", folderId: "nope", name: "A" })).toMatchObject({ ok: false });
    expect(await client.request("folder.create", { projectId: "proj-alpha", conversationIds: ["sess-sandbox-stopped"] })).toMatchObject({ ok: false });
  });

  it("a read-only device sees the folders but every folder method answers read_only", async () => {
    const { client } = await setup({ permission: "read" });
    expect(await folders(client)).toHaveLength(1);
    for (const [method, params] of [
      ["folder.create", { projectId: "proj-alpha", conversationIds: ["sess-local-1"] }],
      ["folder.rename", { projectId: "proj-alpha", folderId: "folder-parser", name: "A" }],
      ["folder.move", { projectId: "proj-alpha", conversationId: "sess-local-1", folderId: null }],
      ["folder.delete", { projectId: "proj-alpha", folderId: "folder-parser" }],
    ] as const) {
      expect(await client.request(method, params), method).toMatchObject({ ok: false, code: "read_only" });
    }
    expect(await folders(client)).toHaveLength(1);
  });

  it("a device that shares only another project sees no folder of this one and cannot edit it", async () => {
    const { client, desktop, deviceKey } = await setup();
    desktop.setDevice(fingerprint(deviceKey.publicKey), { selection: { projects: ["proj-beta"], monos: null } });
    expect(await folders(client)).toEqual([]);
    expect(await client.request("folder.delete", { projectId: "proj-alpha", folderId: "folder-parser" })).toMatchObject({ ok: false, code: "not_selected" });
  });
});
