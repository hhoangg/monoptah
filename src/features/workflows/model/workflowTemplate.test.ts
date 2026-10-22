import { describe, expect, it } from "vitest";
import { renderTemplate, shellQuote, templateVars } from "./workflowTemplate";

describe("workflowTemplate", () => {
  it("renders nested paths", () => {
    const scope = { dev: { pr: { url: "https://x/1" } }, inputs: { n: 3, ok: true } };
    expect(renderTemplate("PR {{dev.pr.url}} n={{inputs.n}} ok={{inputs.ok}}", scope)).toBe(
      "PR https://x/1 n=3 ok=true",
    );
  });

  it("renders lists as pretty JSON", () => {
    expect(renderTemplate("{{items}}", { items: ["a", "b"] })).toBe(
      JSON.stringify(["a", "b"], null, 2),
    );
    expect(renderTemplate("{{o}}", { o: { k: 1 } })).toBe(JSON.stringify({ k: 1 }, null, 2));
  });

  it("missing var renders empty", () => {
    expect(renderTemplate("a{{nope.deep}}b", {})).toBe("ab");
    expect(renderTemplate("a{{x.y}}b", { x: null })).toBe("ab");
  });

  it("if block shows on truthy and hides on [] and undefined", () => {
    const tpl = "A{{#if list}}[{{list}}]{{/if}}B";
    expect(renderTemplate(tpl, { list: ["x"] })).toContain("[");
    expect(renderTemplate(tpl, { list: [] })).toBe("AB");
    expect(renderTemplate(tpl, {})).toBe("AB");
    expect(renderTemplate("{{#if s}}yes{{/if}}", { s: "text" })).toBe("yes");
    expect(renderTemplate("{{#if s}}yes{{/if}}", { s: "" })).toBe("");
    expect(renderTemplate("{{#if s}}yes{{/if}}", { s: false })).toBe("");
  });

  it("templateVars lists all vars once in order", () => {
    const tpl = "{{dev.pr_url}} {{#if item.title}}{{item.title}} {{inputs.project}}{{/if}} {{dev.pr_url}}";
    expect(templateVars(tpl)).toEqual(["dev.pr_url", "item.title", "inputs.project"]);
  });

  it('shell mode quotes "; rm -rf ~" as one literal', () => {
    expect(renderTemplate("echo {{x}}", { x: "a'b; rm -rf ~" }, { shell: true })).toBe(
      "echo 'a'\\''b; rm -rf ~'",
    );
  });

  it("shell mode quotes $() and backticks", () => {
    expect(renderTemplate("echo {{x}}", { x: "$(whoami) `id`" }, { shell: true })).toBe(
      "echo '$(whoami) `id`'",
    );
  });

  it("shell mode leaves literal template text unquoted", () => {
    expect(renderTemplate("git -C {{dir}} status", { dir: "my dir" }, { shell: true })).toBe(
      "git -C 'my dir' status",
    );
  });

  it("shellQuote wraps in single quotes", () => {
    expect(shellQuote("abc")).toBe("'abc'");
    expect(shellQuote("")).toBe("''");
  });
});
