import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { onBundleApplied } from "../../src/pages/transfer/openBundle";
import { rereadIfReplaced } from "./replaced";

let reread: ReturnType<typeof vi.fn>;
let off: () => void;

beforeEach(() => {
  vi.useFakeTimers();
  reread = vi.fn();
  off = onBundleApplied(reread);
});

afterEach(() => {
  off();
  vi.useRealTimers();
});

it("a device's edit the server took makes an open page re-read its character, once for a burst", () => {
  rereadIfReplaced({ event: "party://characters-changed", payload: { ids: ["c_a"], replaced: true } });
  rereadIfReplaced({ event: "party://characters-changed", payload: { ids: ["c_b"], replaced: true } });
  expect(reread).not.toHaveBeenCalled();
  vi.advanceTimersByTime(1000);
  expect(reread).toHaveBeenCalledTimes(1);
});

it("a save of the page's own and other events re-read nothing", () => {
  rereadIfReplaced({ event: "party://characters-changed", payload: { ids: ["c_a"] } });
  rereadIfReplaced({ event: "borealis://content-reloaded", payload: { replaced: true } });
  rereadIfReplaced({ event: "party://characters-changed", payload: null });
  vi.advanceTimersByTime(1000);
  expect(reread).not.toHaveBeenCalled();
});
