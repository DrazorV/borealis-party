import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProxySettings } from "../api/party";

const server = vi.hoisted(() => ({
  now: null as unknown as ProxySettings,
  saves: [] as (string[] | null)[],
  refuse: null as string | null,
}));

const behindNpm: ProxySettings = {
  proxies: [],
  source: "none",
  configFile: [],
  peer: "172.20.20.100",
  peerTrusted: false,
  headers: { cfConnectingIp: "203.0.113.5", xForwardedFor: "203.0.113.5" },
  clientIp: "172.20.20.100",
};

/** What the server answers for a saved list: the peer is trusted when an entry is it (ranges: by prefix, enough here). */
function answer(list: string[] | null): ProxySettings {
  const proxies = list ?? [];
  const trusted = proxies.some((e) => e === behindNpm.peer || (e.includes("/") && behindNpm.peer.startsWith(e.split("/")[0]!.replace(/\.0+$/, ""))));
  return { ...behindNpm, proxies, source: list ? "admin" : "none", peerTrusted: trusted, clientIp: trusted ? "203.0.113.5" : behindNpm.peer };
}

vi.mock("../api/party", () => ({
  adminProxies: async () => server.now,
  saveAdminProxies: async (list: string[] | null) => {
    server.saves.push(list);
    if (server.refuse) throw { code: "invalid_argument", message: server.refuse };
    server.now = answer(list);
    return server.now;
  },
}));

const { TrustedProxies, proxyProblem, parseProxies } = await import("./TrustedProxies");

beforeEach(() => {
  server.now = behindNpm;
  server.saves = [];
  server.refuse = null;
});

const inForce = () => screen.getByRole("list", { name: "Trusted proxies in force" });

describe("TrustedProxies", () => {
  it("shows what the server sees and trusts the proxy in one tap", async () => {
    render(<TrustedProxies />);
    expect(await screen.findByText("Requests arrive from")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("through a proxy at 172.20.20.100 that the server doesn't trust");
    expect(screen.getByText("None: no proxy is trusted.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Trust this proxy (172.20.20.100)" }));
    await waitFor(() => expect(screen.getByText("Saved; it applies now. The server sees your request as coming from 203.0.113.5.")).toBeTruthy());
    expect(server.saves).toEqual([["172.20.20.100"]]);
    expect(screen.getByText(/Working: 172.20.20.100 is trusted, and the server sees your own address, 203.0.113.5/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Trust this proxy/ })).toBeNull();
    expect(within(inForce()).getByText("172.20.20.100")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Use config.toml instead" })).toBeTruthy();
  });

  it("adds several typed entries, IPs and ranges, and removes one", async () => {
    render(<TrustedProxies />);
    const field = await screen.findByLabelText("Add a trusted proxy");
    const add = screen.getByRole("button", { name: "Add" }) as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    fireEvent.change(field, { target: { value: "10.9.9.9" } });
    fireEvent.click(add);
    await waitFor(() => expect(within(inForce()).getByText("10.9.9.9")).toBeTruthy());
    expect((field as HTMLInputElement).value).toBe("");
    // Several at once, a repeat and an IPv6 address among them; the second proxy is the one this request came through.
    fireEvent.change(field, { target: { value: "2001:db8::7, 172.20.20.0/24 10.9.9.9" } });
    fireEvent.click(add);
    await waitFor(() => expect(within(inForce()).getByText("172.20.20.0/24")).toBeTruthy());
    expect(server.saves.at(-1)).toEqual(["10.9.9.9", "2001:db8::7", "172.20.20.0/24"]);
    expect(screen.getByText(/the server sees your own address, 203.0.113.5/)).toBeTruthy();
    expect(within(inForce()).getAllByRole("listitem")).toHaveLength(3);

    fireEvent.click(screen.getByRole("button", { name: "Remove 172.20.20.0/24" }));
    await waitFor(() => expect(within(inForce()).queryByText("172.20.20.0/24")).toBeNull());
    expect(server.saves.at(-1)).toEqual(["10.9.9.9", "2001:db8::7"]);
    expect(screen.getByRole("button", { name: "Trust this proxy (172.20.20.100)" })).toBeTruthy();
  });

  it("checks an entry as it is typed, and shows the server's refusal", async () => {
    render(<TrustedProxies />);
    const field = await screen.findByLabelText("Add a trusted proxy");
    const add = screen.getByRole("button", { name: "Add" }) as HTMLButtonElement;
    fireEvent.change(field, { target: { value: "0.0.0.0/0" } });
    expect(screen.getByText(/0.0.0.0\/0 would trust every address/)).toBeTruthy();
    expect(add.disabled).toBe(true);
    fireEvent.change(field, { target: { value: "npm" } });
    expect(screen.getByText(/npm is not an address/)).toBeTruthy();
    fireEvent.change(field, { target: { value: "172.18.0.0/16" } });
    expect(add.disabled).toBe(false);
    server.refuse = "`172.18.0.0/16` is not allowed here.";
    fireEvent.click(add);
    expect(await screen.findByText("`172.18.0.0/16` is not allowed here.")).toBeTruthy();
    expect((field as HTMLInputElement).value).toBe("172.18.0.0/16");
  });

  it("says when the request seems to come straight in, and where the list comes from", async () => {
    server.now = { ...behindNpm, peer: "192.168.1.20", clientIp: "192.168.1.20", headers: { cfConnectingIp: null, xForwardedFor: null }, source: "config", proxies: ["172.20.20.100"], configFile: ["172.20.20.100"] };
    render(<TrustedProxies />);
    expect((await screen.findByRole("status")).textContent).toContain("no forwarding headers");
    expect(screen.getAllByText("not on your request")).toHaveLength(2);
    expect(screen.getByText(/From config.toml or BOREALIS__SERVER__TRUSTED_PROXIES: 172.20.20.100/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Use config.toml instead" })).toBeNull();
  });
});

describe("proxyProblem", () => {
  it("takes addresses and ranges and refuses everyone", () => {
    for (const ok of ["172.20.20.100", "172.18.0.0/16", "10.0.0.0/8", "::1", "fd00::/32", "2001:db8::7/128"]) expect(proxyProblem(ok)).toBeNull();
    expect(proxyProblem("0.0.0.0/0")).toContain("every address");
    expect(proxyProblem("::/0")).toContain("every address");
    expect(proxyProblem("10.0.0.0/4")).toContain("too wide");
    expect(proxyProblem("npm")).toContain("not an address");
    expect(proxyProblem("300.1.1.1")).toContain("not an address");
    expect(proxyProblem("10.0.0.0/40")).toContain("prefix length");
    expect(parseProxies(" 172.20.20.100,\n\n172.20.20.100 10.0.0.1 ")).toEqual(["172.20.20.100", "10.0.0.1"]);
  });
});
