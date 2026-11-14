// @vitest-environment node
// (node: the vite config loads esbuild, which jsdom's TextEncoder breaks)
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";
import { cargoVersion, releaseVersion } from "./version";

const cargo = () => readFileSync(join(__dirname, "../../party/Cargo.toml"), "utf8");
const file = () => readFileSync(join(__dirname, "../../party/VERSION"), "utf8");

it("reads the party version from party/Cargo.toml, as the desktop build and the release workflow do", () => {
  expect(cargoVersion(cargo())).toBe(/^version = "(.*)"/m.exec(cargo())![1]);
  expect(cargoVersion('[workspace]\nmembers = []\n\n[workspace.package]\nversion = "0.2.0"\nedition = "2021"\n')).toBe("0.2.0");
  expect(() => cargoVersion("[workspace]\n")).toThrow(/no version/);
});

it("reads the release from party/VERSION: the Cargo version, or it and a 4th number for a server release", () => {
  const toml = '[workspace.package]\nversion = "0.3.3"\n';
  expect(releaseVersion("0.3.3\n", toml)).toBe("0.3.3");
  expect(releaseVersion("0.3.3.1\r\n", toml)).toBe("0.3.3.1");
  expect(releaseVersion("0.3.3.12", toml)).toBe("0.3.3.12");
  expect(() => releaseVersion("0.3.4", toml)).toThrow(/must be 0\.3\.3 or 0\.3\.3\.N/);
  expect(() => releaseVersion("0.3.31", toml)).toThrow(/must be 0\.3\.3 or/);
  expect(() => releaseVersion("0.3.3.0", toml)).toThrow(/4th number/);
  expect(() => releaseVersion("0.3.3.x", toml)).toThrow(/4th number/);
  expect(releaseVersion(file(), cargo())).toMatch(/^\d+\.\d+\.\d+(\.[1-9]\d*)?$/);
});

it("the party build shows the party version, not upstream's", async () => {
  const { default: config, PARTY_VERSION } = await import("./vite.config");
  const upstream = (JSON.parse(readFileSync(join(__dirname, "../package.json"), "utf8")) as { version: string }).version;
  expect(PARTY_VERSION).toBe(releaseVersion(file(), cargo()));
  expect(PARTY_VERSION).not.toBe(upstream);
  expect(config.define?.__APP_VERSION__).toBe(JSON.stringify(PARTY_VERSION));
});
