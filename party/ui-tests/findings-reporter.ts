import { writeFileSync } from "node:fs";
import type { FullResult, Reporter, TestCase, TestResult } from "@playwright/test/reporter";

/**
 * Collects the `finding` annotations tests add (glitches reported without failing, such as small tap targets) and the
 * expected failures, and writes them to findings.md: each glitch once per phone, with the screens it was seen on.
 */
export default class FindingsReporter implements Reporter {
  /** project -> finding -> screens */
  private findings = new Map<string, Map<string, Set<string>>>();
  private expected = new Map<string, Set<string>>();

  onTestEnd(test: TestCase, result: TestResult): void {
    const project = test.parent.project()?.name ?? "";
    const title = test.titlePath().slice(3).join(" > ");
    for (const a of [...test.annotations, ...result.annotations]) {
      if (a.type !== "finding" || !a.description) continue;
      const byFinding = this.findings.get(project) ?? new Map<string, Set<string>>();
      this.findings.set(project, byFinding);
      const screens = byFinding.get(a.description) ?? new Set<string>();
      byFinding.set(a.description, screens);
      screens.add(test.title);
    }
    if (test.expectedStatus === "failed") {
      const why = test.annotations.find((a) => a.type === "fail")?.description ?? "";
      const outcome = result.status === "failed" ? "still fails" : "PASSES NOW: remove its test.fail";
      const items = this.expected.get(project) ?? new Set<string>();
      this.expected.set(project, items);
      items.add(`${title} (${outcome})${why ? `: ${why}` : ""}`);
    }
  }

  onEnd(_result: FullResult): void {
    const lines = ["# Phone UI findings", "", "Written by party/ui-tests on every run. Not committed.", ""];
    lines.push("## Expected failures (fixes planned)", "");
    if (this.expected.size === 0) lines.push("None.", "");
    for (const [project, items] of [...this.expected].sort()) {
      lines.push(`### ${project}`, "", ...[...items].sort().map((i) => `- ${i}`), "");
    }
    lines.push("## Findings (reported, not failing)", "");
    if (this.findings.size === 0) lines.push("None.", "");
    let count = 0;
    for (const [project, byFinding] of [...this.findings].sort()) {
      lines.push(`### ${project}`, "");
      for (const [text, screens] of [...byFinding].sort()) {
        count++;
        lines.push(`- ${text} — ${[...screens].sort().join(", ")}`);
      }
      lines.push("");
    }
    writeFileSync("findings.md", `${lines.join("\n")}\n`);
    const expected = [...this.expected.values()].reduce((n, s) => n + s.size, 0);
    console.log(`\nPhone UI findings: ${count} distinct, ${expected} expected failures. See party/ui-tests/findings.md`);
  }

  printsToStdio(): boolean { return false; }
}
