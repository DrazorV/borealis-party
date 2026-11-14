import type { APIRequestContext, Page } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect } from "./fixtures";

/**
 * A spec's own account (`<prefix>-<project>`), made on its first run and signed into after, for specs that need an
 * account nothing else touches (a new account's first run, its import results). Test values only.
 */
export async function ownAccount(request: APIRequestContext, baseURL: string, prefix: string, project: string): Promise<{ token: string; user: unknown }> {
  const name = `${prefix}-${project}`.replace(/[^a-z0-9-]/gi, "").toLowerCase();
  const password = "own-account-test-4417";
  const login = () => request.post(`${baseURL}/api/auth/login`, { data: { username: name, password } });
  let r = await login();
  if (!r.ok()) {
    let reg = await request.post(`${baseURL}/api/auth/register`, { data: { username: name, email: `${name}@phone-rig.test`, password } });
    for (let i = 0; i < 3 && reg.status() === 500; i++) {
      await new Promise((done) => setTimeout(done, 500 * (i + 1)));
      reg = await request.post(`${baseURL}/api/auth/register`, { data: { username: name, email: `${name}@phone-rig.test`, password } });
    }
    expect(reg.ok() || reg.status() === 409, `register: ${reg.status()} ${await reg.text()}`).toBeTruthy();
    const out = (reg.ok() ? await reg.json() : {}) as { token?: string };
    if (!out.token) {
      // Sign-ups wait for an admin on this server: approve it.
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
  return (await r.json()) as { token: string; user: unknown };
}

export async function rpc<T = unknown>(request: APIRequestContext, baseURL: string, token: string, command: string, args: unknown): Promise<T> {
  const r = await request.post(`${baseURL}/api/rpc/${command}`, { data: args, headers: { authorization: `Bearer ${token}` } });
  expect(r.ok(), `${command}: ${r.status()} ${await r.text()}`).toBeTruthy();
  return r.json() as Promise<T>;
}

/** The page signed in as `session` on its first load. */
export async function signInAs(page: Page, session: { token: string; user: unknown }): Promise<void> {
  await page.addInitScript((s) => {
    if (!sessionStorage.getItem("phone-rig.seeded")) {
      localStorage.setItem("party.session", s);
      sessionStorage.setItem("phone-rig.seeded", "1");
    }
  }, JSON.stringify({ baseUrl: "", ...session }));
}
