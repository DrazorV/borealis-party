import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Page } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, measureOverflow, minTap, open, signedInShell, test } from "./fixtures";

/**
 * Additional Content on a phone: Remove all on a pack group asks with the exact count and removes the group's packs
 * in one request (party/crates/borealis-server/src/pack_removal.rs). Each project has an account of its own, so no
 * other test's content is touched.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const PACK = readFileSync(join(FIXTURES, "d4-pack.xml")).toString("base64");

/**
 * This project's own account (`rm-<project>`), made on its first run and signed into after, with no custom packs left
 * from an earlier repeat. Sign-ups are rate limited per address, so repeats reuse it.
 */
async function ownAccount(request: APIRequestContext, baseURL: string, project: string): Promise<{ token: string; user: unknown }> {
  const name = `rm-${project}`.replace(/[^a-z0-9-]/gi, "").toLowerCase();
  const password = "remove-all-test-7731";
  const login = () => request.post(`${baseURL}/api/auth/login`, { data: { username: name, password } });
  let r = await login();
  if (!r.ok()) {
    let reg = await request.post(`${baseURL}/api/auth/register`, { data: { username: name, email: `${name}@phone-rig.test`, password } });
    // Sign-ups at the same moment from parallel workers have answered 500 on the rig's SQLite server: try again.
    for (let i = 0; i < 3 && reg.status() === 500; i++) {
      await new Promise((done) => setTimeout(done, 500 * (i + 1)));
      reg = await request.post(`${baseURL}/api/auth/register`, { data: { username: name, email: `${name}@phone-rig.test`, password } });
    }
    // 409: another worker made it a moment ago.
    expect(reg.ok() || reg.status() === 409, `register: ${reg.status()} ${await reg.text()}`).toBeTruthy();
    const out = (reg.ok() ? await reg.json() : { token: "taken" }) as { token?: string };
    if (!out.token) {
      const admin = await request.post(`${baseURL}/api/auth/login`, { data: { username: "admin", password: ACCOUNTS.admin.password } });
      const auth = { authorization: `Bearer ${((await admin.json()) as { token: string }).token}` };
      const list = (await (await request.get(`${baseURL}/api/admin/accounts?perPage=200`, { headers: auth })).json()) as { items?: { id: string; username: string }[] };
      const id = list.items?.find((a) => a.username === name)?.id;
      const approve = await request.post(`${baseURL}/api/admin/accounts/${id}/status`, { data: { status: "active" }, headers: auth });
      expect(approve.ok(), `approve: ${await approve.text()}`).toBeTruthy();
    }
    r = await login();
  }
  expect(r.ok(), `login: ${r.status()} ${await r.text()}`).toBeTruthy();
  const session = (await r.json()) as { token: string; user: unknown };
  const packs = (await rpc(request, baseURL, session.token, "list_packs", {})) as { id: string; kind: string }[];
  const left = packs.filter((p) => p.kind !== "bundled").map((p) => p.id);
  if (left.length > 0) {
    const out = await request.post(`${baseURL}/api/content/remove-packs`, { data: { packIds: left }, headers: { authorization: `Bearer ${session.token}` } });
    expect(out.ok(), await out.text()).toBeTruthy();
  }
  return session;
}

async function rpc(request: APIRequestContext, baseURL: string, token: string, command: string, args: unknown) {
  const r = await request.post(`${baseURL}/api/rpc/${command}`, { data: args, headers: { authorization: `Bearer ${token}` } });
  expect(r.ok(), `${command}: ${r.status()} ${await r.text()}`).toBeTruthy();
  return r.json() as Promise<unknown>;
}

async function signInAs(page: Page, session: { token: string; user: unknown }) {
  await page.addInitScript((s) => {
    if (!sessionStorage.getItem("phone-rig.seeded")) {
      localStorage.setItem("party.session", s);
      sessionStorage.setItem("phone-rig.seeded", "1");
    }
  }, JSON.stringify({ baseUrl: "", ...session }));
}

test("Remove all on a pack group asks with the count and removes the group at once", async ({ page, request, baseURL }, testInfo) => {
  const session = await ownAccount(request, baseURL!, testInfo.project.name);
  for (const name of ["remove-a.xml", "remove-b.xml", "remove-c.xml"]) {
    await rpc(request, baseURL!, session.token, "import_pack", { request: { fileName: name, contentBase64: PACK } });
  }
  await signInAs(page, session);
  await open(page, "/content");
  await signedInShell(page);

  const removeAll = page.getByRole("button", { name: "Remove all Custom packs" });
  await removeAll.scrollIntoViewIfNeeded();
  await expect(page.getByRole("button", { name: "Custom (3)" })).toBeVisible();
  await expect(page.getByRole("button", { name: /Remove all Built-in/ })).toHaveCount(0);
  const box = (await removeAll.boundingBox())!;
  expect(Math.min(box.width, box.height), "Remove all tap target").toBeGreaterThanOrEqual(minTap(testInfo) - 0.5);
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  await page.screenshot({ path: testInfo.outputPath("remove-all.png") });

  await removeAll.tap();
  const dialog = page.getByRole("dialog", { name: "Remove all" });
  await expect(dialog).toContainText("Remove all 3 Custom packs?");
  await expect(dialog).toContainText("show it as missing content");
  await expect(dialog.getByRole("button", { name: "Remove 3" })).toBeInViewport();
  await page.waitForTimeout(400); // the sheet's slide-in, for the screenshot only
  await page.screenshot({ path: testInfo.outputPath("remove-all-dialog.png") });
  await dialog.getByRole("button", { name: "Remove 3" }).tap();

  await expect(page.getByRole("button", { name: /^Custom \(/ })).toHaveCount(0, { timeout: 20_000 });
  const packs = (await rpc(request, baseURL!, session.token, "list_packs", {})) as { kind: string }[];
  expect(packs.filter((p) => p.kind !== "bundled")).toEqual([]);
});
