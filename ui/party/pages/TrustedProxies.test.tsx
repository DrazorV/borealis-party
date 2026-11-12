import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProxySettings } from "../api/party";

const server = vi.hoisted(() => ({
  now: null as unknown as ProxySettings,
  saves: [] as (string[] | null)[],
  refuse: null as string | null,
  after: null as ((list: string[] | null) => ProxySettings) | null,
}));

vi.mock("../api/party", () => ({
  adminProxies: async () => server.now,
  saveAdminProxies: async (list: string[] | null) => {
    server.saves.push(list);
    if (server.refuse) throw { code: "invalid_argument", message: server.refuse };
    server.now = server.after ? server.after(list) : server.now;
    return server.now;
  },
}));

const { TrustedProxies, proxyProblem, parseProxies } = await import("./TrustedProxies");

const behindNpm: ProxySettings = {
  proxies: [],
  source: "none",
  configFile: [],
  peer: "172.20.20.100",
  peerTrusted: false,
  headers: { cfConnectingIp: "203.0.113.5", xForwardedFor: "203.0.113.5" },
  clientIp: "172.20.20.100",
};

beforeEach(() => {
  server.now = behindNpm;
  server.saves = [];
  server.refuse = null;
  server.after = (list) => ({ ...behindNpm, proxies: list ?? [], source: list ? "admin" : "none", peerTrusted: !!list?.includes("172.20.20.100"), clientIp: list?.includes("172.20.20.100") ? "203.0.113.5" : "172.20.20.100" });
});

describe("TrustedProxies", () => {
  it("shows what the server sees and trusts the proxy in one tap", async () => {
    render(<TrustedProxies />);
    expect(await screen.findByText("Requests arrive from")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("through a proxy at 172.20.20.100 that the server doesn't trust");
    expect(screen.getAllByText("203.0.113.5")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Trust this proxy (172.20.20.100)" }));
    await waitFor(() => expect(screen.getByText("Saved; it applies now. The server sees your request as coming from 203.0.113.5.")).toBeTruthy());
    expect(server.saves).toEqual([["172.20.20.100"]]);
    expect(screen.getByText(/Working: 172.20.20.100 is trusted, and the server sees your own address, 203.0.113.5/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Trust this proxy/ })).toBeNull();
    expect((screen.getByLabelText("Trusted proxies") as HTMLTextAreaElement).value).toBe("172.20.20.100");
    expect(screen.getByRole("button", { name: "Use config.toml instead" })).toBeTruthy();
  });

  it("checks a typed list before saving, and shows the server's refusal", async () => {
    render(<TrustedProxies />);
    const field = await screen.findByLabelText("Trusted proxies");
    const saveList = screen.getByRole("button", { name: "Save list" }) as HTMLButtonElement;
    expect(saveList.disabled).toBe(true);
    fireEvent.change(field, { target: { value: "172.20.20.100\n0.0.0.0/0" } });
    expect(screen.getByText(/0.0.0.0\/0 would trust every address/)).toBeTruthy();
    expect(saveList.disabled).toBe(true);
    fireEvent.change(field, { target: { value: "172.20.20.100\n172.18.0.0/16" } });
    expect(saveList.disabled).toBe(false);
    server.refuse = "`172.18.0.0/16` is not allowed here.";
    fireEvent.click(saveList);
    expect(await screen.findByText("`172.18.0.0/16` is not allowed here.")).toBeTruthy();
    expect(server.saves).toEqual([["172.20.20.100", "172.18.0.0/16"]]);
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
