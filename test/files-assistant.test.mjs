import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import test from "node:test";

import {
  loginSession,
  startDarWithOidc,
  stopDarWithOidc,
  waitForHealth,
} from "./helpers/dar-server.mjs";

test("files search, stats, diff, download, assistant tools, and widget work over http", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-files-stage6-test-");
  const context = await startDarWithOidc(dataRoot);
  try {
    await waitForHealth(context.port, context.running.child, context.running.output);
    const session = await loginSession(context.baseUrl, context.running.output);
    const base = context.baseUrl;
    const cookies = { cookie: session.cookies };
    const upload = async (path, body) => {
      const response = await fetch(`${base}/api/v1/files/upload?path=${encodeURIComponent(path)}`, {
        method: "POST",
        headers: {
          cookie: session.cookies,
          origin: base,
          "x-csrf-token": session.csrfToken,
        },
        body,
      });
      assert.equal(response.status, 201, context.running.output());
      return response;
    };
    const getJson = async (path, expect = 200) => {
      const response = await fetch(`${base}${path}`, { headers: cookies });
      assert.equal(response.status, expect, context.running.output());
      return response.json();
    };

    await upload("/Docs/notes.md", "first draft");
    await upload("/Docs/pic.png", "0123456789");
    await upload("/Docs/todo.txt", "buy milk");

    // Search matches names case-insensitively and filters by type.
    const search = await getJson("/api/v1/files/search?q=NOTE");
    assert.equal(search.total, 1);
    assert.equal(search.items[0].path, "/Docs/notes.md");

    const documents = await getJson("/api/v1/files/search?type=document");
    assert.equal(documents.total, 2);

    const paged = await getJson(
      `/api/v1/files/search?folder=${encodeURIComponent("/Docs")}&limit=1&offset=2`,
    );
    assert.equal(paged.total, 3);
    assert.equal(paged.items.length, 1);

    // Stats report usage against the configured quota.
    const stats = await getJson("/api/v1/files/stats");
    assert.equal(typeof stats.used, "number");
    assert.ok(stats.used >= 25, context.running.output());
    assert.equal(typeof stats.total, "number");

    // Version diffs return the baseline line diff shape.
    const editorPath = "/api/v1/files/editor?path=%2FDocs%2Fnotes.md";
    const read = await fetch(`${base}${editorPath}`, { headers: cookies });
    const doc = await read.json();
    const save = await fetch(`${base}${editorPath}`, {
      method: "PUT",
      headers: {
        ...cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
        "content-type": "application/json",
        "if-match": `"${doc.etag}"`,
      },
      body: JSON.stringify({ content: "second draft" }),
    });
    assert.equal(save.status, 200, context.running.output());
    const versions = await getJson("/api/v1/files/versions?path=%2FDocs%2Fnotes.md");
    const version = versions.items[0];
    const diff = await getJson(
      `/api/v1/files/versions/diff?path=%2FDocs%2Fnotes.md&v1=${version.id}&v2=${version.id}`,
    );
    assert.match(diff.diff, /first draft/);

    // Download returns raw bytes with an attachment disposition.
    const download = await fetch(
      `${base}/api/v1/files/download?path=${encodeURIComponent("/Docs/notes.md")}`,
      { headers: cookies },
    );
    assert.equal(download.status, 200, context.running.output());
    assert.equal(download.headers.get("content-disposition"), 'attachment; filename="notes.md"');
    assert.equal(await download.text(), "second draft");
    const folderDownload = await fetch(
      `${base}/api/v1/files/download?path=${encodeURIComponent("/Docs")}`,
      { headers: cookies },
    );
    assert.equal(folderDownload.status, 400, context.running.output());

    // The assistant catalog exposes the Files tools.
    const tools = await getJson("/api/v1/assistant/tools");
    const toolIds = tools.items.map((tool) => tool.name);
    for (const id of ["files.list", "files.search", "files.read", "files.write-text",
      "files.create-folder", "files.rename", "files.move", "files.copy", "files.favorite",
      "files.tag", "files.share", "files.trash", "files.restore", "files.delete-permanently"]) {
      assert.ok(toolIds.includes(id), `${id} missing from ${toolIds.join(",")}`);
    }

    const invoke = async (id, arguments_, confirm = false) => {
      const response = await fetch(`${base}/api/v1/assistant/tools/${id}/invoke`, {
        method: "POST",
        headers: {
          ...cookies,
          origin: base,
          "x-csrf-token": session.csrfToken,
          "content-type": "application/json",
        },
        body: JSON.stringify({ arguments: arguments_, confirm }),
      });
      return response;
    };

    // Read tools answer immediately.
    const list = await invoke("files.list", { path: "/Docs" });
    assert.equal(list.status, 200, context.running.output());
    const listed = await list.json();
    assert.equal(listed.path, "/Docs");
    assert.equal(listed.items.length, 3);

    const searchTool = await invoke("files.search", { q: "todo", type: "document" });
    assert.equal(searchTool.status, 200, context.running.output());
    assert.equal((await searchTool.json()).items[0].path, "/Docs/todo.txt");

    const readTool = await invoke("files.read", { path: "/Docs/notes.md" });
    assert.equal(readTool.status, 200, context.running.output());
    assert.equal((await readTool.json()).content, "second draft");

    // Safe-write tools mutate on behalf of the caller.
    const write = await invoke("files.write-text", {
      path: "/Docs/assistant.md",
      content: "written by a tool",
    });
    assert.equal(write.status, 200, context.running.output());
    const verify = await getJson("/api/v1/files/editor?path=%2FDocs%2Fassistant.md");
    assert.equal(verify.content, "written by a tool");

    const folder = await invoke("files.create-folder", { path: "/Docs/Nested" });
    assert.equal(folder.status, 200, context.running.output());

    const rename = await invoke("files.rename", {
      path: "/Docs/assistant.md",
      name: "renamed.md",
    });
    assert.equal(rename.status, 200, context.running.output());

    const move = await invoke("files.move", {
      from: "/Docs/renamed.md",
      to: "/Docs/Nested/renamed.md",
    });
    assert.equal(move.status, 200, context.running.output());

    const copy = await invoke("files.copy", {
      from: "/Docs/Nested/renamed.md",
      to: "/Docs/copied.md",
    });
    assert.equal(copy.status, 200, context.running.output());

    const favorite = await invoke("files.favorite", {
      path: "/Docs/copied.md",
      favorite: true,
    });
    assert.equal(favorite.status, 200, context.running.output());

    // Destructive tools need confirmation; with it they trash the file.
    const unconfirmed = await invoke("files.trash", { path: "/Docs/copied.md" });
    assert.equal(unconfirmed.status, 409, context.running.output());
    const confirmed = await invoke(
      "files.trash",
      { path: "/Docs/copied.md" },
      true,
    );
    assert.equal(confirmed.status, 200, context.running.output());

    const trashed = await confirmed.json();
    assert.ok(trashed.id, context.running.output());

    const restore = await invoke("files.restore", { id: trashed.id });
    assert.equal(restore.status, 200, context.running.output());

    const reTrash = await invoke("files.trash", { path: "/Docs/copied.md" }, true);
    assert.equal(reTrash.status, 200, context.running.output());
    const reTrashed = await (await reTrash).json();
    const purge = await invoke("files.delete-permanently", { id: reTrashed.id }, true);
    assert.equal(purge.status, 200, context.running.output());

    // Invalid arguments answer 400 with the humanized fields.
    const invalid = await invoke("files.favorite", { path: "/Docs/copied.md" });
    assert.equal(invalid.status, 400, context.running.output());

    // The shared picker browses and validates selections over http.
    const pickerPage = await getJson(
      "/api/v1/files/picker?path=%2FDocs&selection=files&mime_prefix=image%2F",
    );
    assert.equal(pickerPage.path, "/Docs");
    // Folders stay for navigation; only the image file matches the prefix.
    const names = pickerPage.items.map((entry) => entry.name);
    assert.ok(names.includes("pic.png"), context.running.output());
    assert.ok(names.includes("assistant.md") === false, context.running.output());

    const validated = await fetch(`${base}/api/v1/files/picker/validate`, {
      method: "POST",
      headers: {
        ...cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
        "content-type": "application/json",
      },
      body: JSON.stringify({
        paths: ["/Docs/notes.md"],
        selection: "files",
        multiple: false,
      }),
    });
    assert.equal(validated.status, 200, context.running.output());
    const validatedItems = (await validated.json()).items;
    assert.equal(validatedItems.length, 1);
    assert.equal(validatedItems[0].path, "/Docs/notes.md");

    const typeMismatch = await fetch(`${base}/api/v1/files/picker/validate`, {
      method: "POST",
      headers: {
        ...cookies,
        origin: base,
        "x-csrf-token": session.csrfToken,
        "content-type": "application/json",
      },
      body: JSON.stringify({ paths: ["/Docs"], selection: "files" }),
    });
    assert.equal(typeMismatch.status, 400, context.running.output());
    assert.equal((await typeMismatch.json()).error.code, "invalid");

    // Mutations land in the user activity feed.
    const feed = await getJson("/api/v1/activity");
    const eventTypes = feed.items.map((event) => event.eventType);
    for (const type of ["fileUploaded", "fileTrashed", "fileRestored"]) {
      assert.ok(eventTypes.includes(type), `${type} missing from ${eventTypes.join(",")}`);
    }
    assert.ok(feed.items.every((event) => event.appId === "files"), context.running.output());

    // The dashboard catalog carries the Files favorites widget and its data.
    const catalog = await getJson("/api/v1/dashboard/catalog");
    assert.ok(
      catalog.widgets.some((widget) => widget.id === "files.favorites"),
      context.running.output(),
    );
    const widgetData = await getJson(
      "/api/v1/dashboard/widgets/files.favorites?size=standard",
    );
    assert.equal(widgetData.kind, "list");
    assert.equal(widgetData.items.length, 0);
    assert.equal(widgetData.emptyState, "No favorite files");
  } finally {
    await stopDarWithOidc(context);
    await rm(dataRoot, { recursive: true, force: true });
  }
});
