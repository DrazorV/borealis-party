import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { memoryStorage } from "../test/memoryStorage";

const env = vi.hoisted(() => ({ tauri: false, folder: "https://github.com/DrazorV/borealis-party/releases/latest/download" as string | null, asked: 0 }));
vi.mock("../../src/lib/tauri", () => ({ isTauri: () => env.tauri }));
vi.mock("../api/party", () => ({
  checkServer: async () => { env.asked += 1; return { app: "borealis-party", appDownload: env.folder }; },
}));

const { AppOffer, forgetDownloadFolderForTests } = await import("./AppOffer");

const PIXEL = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Mobile Safari/537.36";
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
const LINUX = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";
const CHROMEBOOK = "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";
const WINDOWS = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";

const device = (userAgent: string) => vi.spyOn(navigator, "userAgent", "get").mockReturnValue(userAgent);

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  env.tauri = false;
  env.folder = "https://github.com/DrazorV/borealis-party/releases/latest/download";
  env.asked = 0;
  forgetDownloadFolderForTests();
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("offers the APK on an Android browser, with the address to enter", async () => {
  device(PIXEL);
  render(<AppOffer />);
  const link = await screen.findByRole("link", { name: /download the app/i });
  expect(link.getAttribute("href")).toBe("https://github.com/DrazorV/borealis-party/releases/latest/download/borealis-party-arm64.apk");
  expect(screen.getByText(window.location.origin.replace(/^https?:\/\//, ""), { exact: false })).toBeTruthy();
});

it("offers an iPhone the app to sideload, says how, and links the other systems' builds", async () => {
  device(IPHONE);
  render(<AppOffer />);
  expect(await screen.findByText("Get the iPhone and iPad app.")).toBeTruthy();
  expect(screen.getByText(/AltStore, SideStore or Sideloadly/)).toBeTruthy();
  expect(screen.getByRole("link", { name: /Download the app/ }).getAttribute("href"))
    .toBe("https://github.com/DrazorV/borealis-party/releases/latest/download/borealis-party-ios-unsigned.ipa");
  expect(screen.getByRole("link", { name: "Other systems" }).getAttribute("href")).toBe("https://github.com/DrazorV/borealis-party/releases/latest");
});

it("offers Windows its installer", async () => {
  device(WINDOWS);
  render(<AppOffer />);
  expect(await screen.findByText("Get the Windows app.")).toBeTruthy();
  expect(screen.getByRole("link", { name: /download the installer/i }).getAttribute("href")).toBe("https://github.com/DrazorV/borealis-party/releases/latest/download/borealis-party-windows-x64-setup.exe");
});

it("offers Linux the AppImage, then the .deb", async () => {
  device(LINUX);
  render(<AppOffer />);
  await screen.findByText("Get the Linux app.");
  const hrefs = screen.getAllByRole("link").map((a) => a.getAttribute("href"));
  expect(hrefs).toEqual([
    "https://github.com/DrazorV/borealis-party/releases/latest/download/borealis-party-linux-x86_64.AppImage",
    "https://github.com/DrazorV/borealis-party/releases/latest/download/borealis-party-linux-amd64.deb",
    "https://github.com/DrazorV/borealis-party/releases/latest",
  ]);
});

it("shows nothing in the app, on other systems, or when the server offers no download", async () => {
  device(PIXEL);
  env.tauri = true;
  const { container, unmount } = render(<AppOffer />);
  expect(container.innerHTML).toBe("");
  unmount();
  env.tauri = false;
  device(CHROMEBOOK);
  expect(render(<AppOffer />).container.innerHTML).toBe("");
  device(PIXEL);
  expect(render(<AppOffer folder={null} />).container.innerHTML).toBe("");
  expect(env.asked).toBe(0);
});

it("uses the sign-in page's answer instead of asking again", () => {
  device(PIXEL);
  render(<AppOffer folder="https://files.example.com/apps" />);
  expect(screen.getByRole("link", { name: /download the app/i }).getAttribute("href")).toBe("https://files.example.com/apps/borealis-party-arm64.apk");
  expect(env.asked).toBe(0);
});

it("stays closed in this browser once dismissed", async () => {
  device(PIXEL);
  const first = render(<AppOffer />);
  fireEvent.click(await screen.findByRole("button", { name: "Dismiss" }));
  expect(first.container.innerHTML).toBe("");
  first.unmount();
  expect(render(<AppOffer folder="https://files.example.com/apps" />).container.innerHTML).toBe("");
});
