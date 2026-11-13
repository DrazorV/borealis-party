import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { expect, it } from "vitest";
import { NavLink } from "./shellRouter";

it("keeps the shell's own aria-current: one START sub-tab is current on /content/sources", () => {
  render(
    <MemoryRouter initialEntries={["/content/sources"]}>
      <NavLink to="/content/sources" className="tab" aria-current="page">Sources</NavLink>
      <NavLink to="/content" className="tab" aria-current={undefined}>Additional Content</NavLink>
      <NavLink to="/content" className="tab">Router decides</NavLink>
    </MemoryRouter>,
  );
  const sources = screen.getByRole("link", { name: "Sources" });
  const content = screen.getByRole("link", { name: "Additional Content" });
  expect(sources.getAttribute("aria-current")).toBe("page");
  expect(content.getAttribute("aria-current")).not.toBe("page");
  expect(content.className).toBe("tab");
  // Without the prop, NavLink behaves as react-router's own.
  const plain = screen.getByRole("link", { name: "Router decides" });
  expect(plain.getAttribute("aria-current")).toBe("page");
  expect(plain.className).toBe("tab active");
});
