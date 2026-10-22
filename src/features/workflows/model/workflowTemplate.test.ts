import { describe, expect, it } from "vitest";
import { renderTemplate, shellQuote, shellTemplateProblems, templateVars } from "./workflowTemplate";

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

  it("does not follow the prototype chain", () => {
    expect(renderTemplate("[{{inputs.toString}}][{{inputs.__proto__}}]", { inputs: {} })).toBe("[][]");
    expect(renderTemplate("echo {{inputs.toString}}", { inputs: {} }, { shell: true })).toBe("echo ''");
  });

  it("renders non-JSON values as empty", () => {
    expect(renderTemplate("[{{f}}]", { f: () => 1 })).toBe("[]");
  });

  describe("shellTemplateProblems", () => {
    it("flags a var inside double quotes", () => {
      const problems = shellTemplateProblems('echo "{{x}}"');
      expect(problems).toHaveLength(1);
      expect(problems[0]).toContain("{{x}}");
      expect(problems[0]).toContain("double quotes");
    });

    it("flags a var inside single quotes and backticks", () => {
      expect(shellTemplateProblems("echo '{{x}}'")).toHaveLength(1);
      expect(shellTemplateProblems("echo `{{x}}`")).toHaveLength(1);
    });

    it("keeps bare usage unflagged", () => {
      expect(shellTemplateProblems("git commit -m {{msg}}")).toEqual([]);
      expect(shellTemplateProblems("git -C {{dir}} log --format='%h' {{ref}}")).toEqual([]);
    });

    it.each([
      ["a: comment with an apostrophe", "# don't\necho '{{x}}'"],
      ["b: nested quotes in a substitution", 'echo "$(echo "{{x}}")"'],
      ["c: backslash before a var inside quotes", 'echo "a\\{{x}}"'],
      ["c: backslash before a bare var", "echo \\{{x}}"],
      ["d: unquoted heredoc", "cat <<EOF\n{{x}}\nEOF"],
      ["e: ANSI-C string with an escaped quote", "echo $'\\' {{x}} '"],
      ["quote only inside an #if body", "echo {{#if a}}'{{/if}} {{x}}"],
      ["backslash only inside an #if body", "echo {{#if a}}\\{{/if}} {{x}}"],
    ])("flags every var when the text has syntax it cannot follow (%s)", (_name, tpl) => {
      const problems = shellTemplateProblems(tpl);
      expect(problems.some((p) => p.includes("{{x}}"))).toBe(true);
    });

    it("does not flag a bare var", () => {
      expect(shellTemplateProblems("echo {{x}}")).toEqual([]);
    });

    it("does not flag a quote pair that does not enclose a var", () => {
      expect(shellTemplateProblems('echo "hi" {{x}}')).toEqual([]);
      expect(shellTemplateProblems("echo 'a b' {{x}} 'c'")).toEqual([]);
    });

    it("respects backslash escapes outside single quotes", () => {
      expect(shellTemplateProblems('echo \\"{{x}}')).toEqual([]);
      expect(shellTemplateProblems('echo "a\\"{{x}}"')).toHaveLength(1);
      expect(shellTemplateProblems("echo 'a\\' {{x}}")).toEqual([]);
    });

    it("reports each var and ignores if tags", () => {
      expect(shellTemplateProblems('{{#if a}}echo "{{a}} {{b}}"{{/if}}')).toHaveLength(2);
    });
  });
});
