// @vitest-environment node
// (node: the vite config loads esbuild, which jsdom's TextEncoder breaks)
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";
import { cargoVersion } from "./version";

const cargo = () => readFileSync(join(__dirname, "../../party/Cargo.toml"), "utf8");

it("reads the party version from party/Cargo.toml, as the desktop build and the release workflow do", () => {
  expect(cargoVersion(cargo())).toBe(/^version = "(.*)"/m.exec(cargo())![1]);
  expect(cargoVersion('[workspace]\nmembers = []\n\n[workspace.package]\nversion = "0.2.0"\nedition = "2021"\n')).toBe("0.2.0");
  expect(() => cargoVersion("[workspace]\n")).toThrow(/no version/);
});

it("the party build shows the party version, not upstream's", async () => {
  const { default: config, PARTY_VERSION } = await import("./vite.config");
  const upstream = (JSON.parse(readFileSync(join(__dirname, "../package.json"), "utf8")) as { version: string }).version;
  expect(PARTY_VERSION).toBe(cargoVersion(cargo()));
  expect(PARTY_VERSION).not.toBe(upstream);
  expect(config.define?.__APP_VERSION__).toBe(JSON.stringify(PARTY_VERSION));
});
