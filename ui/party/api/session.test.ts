import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { memoryStorage } from "../test/memoryStorage";
import {
  askSignInAgain, dropSignInAgain, isLocalAddress, loadSession, markExpired, normaliseAddress, rememberedServer, saveSession, setServerAddress, wantsSignInAgain,
} from "./session";

beforeEach(() => { vi.stubGlobal("localStorage", memoryStorage()); vi.stubGlobal("sessionStorage", memoryStorage()); });
afterEach(() => { vi.unstubAllGlobals(); });

describe("normaliseAddress", () => {
  it.each([
    ["party.example.com", "https://party.example.com"],
    ["party.example.com:8443/", "https://party.example.com:8443"],
    ["  Party.Example.com/Borealis/ ", "https://party.example.com/Borealis"],
    ["192.168.1.20:8787", "http://192.168.1.20:8787"],
    ["10.0.2.2", "http://10.0.2.2"],
    ["localhost:8787", "http://localhost:8787"],
    ["nas.local:8787", "http://nas.local:8787"],
    ["raspberrypi:8787", "http://raspberrypi:8787"],
    ["[::1]:8787", "http://[::1]:8787"],
    ["http://party.example.com", "http://party.example.com"],
    ["HTTPS://192.168.1.20", "https://192.168.1.20"],
    ["[fe80::1]", "http://[fe80::1]"],
  ])("%s means %s", (typed, url) => {
    expect(normaliseAddress(typed)).toBe(url);
  });
});

describe("isLocalAddress", () => {
  it.each([
    ["http://192.168.1.20:8787", true],
    ["http://localhost", true],
    ["http://nas.local:8787", true],
    ["http://raspberrypi:8787/x.y", true],
    ["http://[::1]:8787", true],
    ["http://party.example.com", false],
    ["https://party.example.com:8443/borealis", false],
  ])("%s is local: %s", (url, local) => {
    expect(isLocalAddress(url)).toBe(local);
  });
});

describe("setServerAddress", () => {
  const user = { id: "u1", username: "alice", email: null, admin: false, mustChangePassword: false };

  it("moves the session and the remembered address, keeping the sign-in", () => {
    saveSession({ baseUrl: "http://192.168.1.20:8787", token: "t", user });
    setServerAddress("https://party.example.com");
    expect(loadSession()).toEqual({ baseUrl: "https://party.example.com", token: "t", user });
    expect(rememberedServer()).toBe("https://party.example.com");
  });

  it("only remembers the address when nobody is signed in", () => {
    setServerAddress("https://party.example.com");
    expect(loadSession()).toBeNull();
    expect(rememberedServer()).toBe("https://party.example.com");
  });
});

describe("signed out by the server", () => {
  const user = { id: "u1", username: "alice", email: null, admin: false, mustChangePassword: false };

  it("an expired session is kept, and says when the account itself was turned off", () => {
    saveSession({ baseUrl: "", token: "t", user });
    markExpired();
    expect(loadSession()).toEqual({ baseUrl: "", token: "t", user, expired: true });
    markExpired(true);
    expect(loadSession()).toEqual({ baseUrl: "", token: "t", user, expired: true, blocked: true });
    // Signing in again replaces both.
    saveSession({ baseUrl: "", token: "t2", user });
    expect(loadSession()).toEqual({ baseUrl: "", token: "t2", user });
  });

  it("'Sign in again' is asked for until signed in or dropped", () => {
    expect(wantsSignInAgain()).toBe(false);
    askSignInAgain();
    expect(wantsSignInAgain()).toBe(true);
    dropSignInAgain();
    expect(wantsSignInAgain()).toBe(false);
  });

  it("another account signing in does not reopen the previous one's character", () => {
    saveSession({ baseUrl: "", token: "t", user });
    sessionStorage.setItem("lk.currentCharacter", "c_1");
    saveSession({ baseUrl: "", token: "t2", user });
    expect(sessionStorage.getItem("lk.currentCharacter")).toBe("c_1");
    saveSession({ baseUrl: "", token: "t3", user: { ...user, id: "u2" } });
    expect(sessionStorage.getItem("lk.currentCharacter")).toBeNull();
  });
});
