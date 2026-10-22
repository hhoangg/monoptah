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
      ["f1: dollar right before a var", "echo ${{x}}"],
      ["f1: dollar and space before a var", "echo ${{ x }}"],
      ["f3: comment after a closing paren", "(true)# don't\necho '{{x}}'"],
      ["f3: comment after an opening paren", "(# don't\necho '{{x}}'"],
      ["arithmetic expansion", "echo $[1+{{x}}]"],
      ["A: line continuation joins $ and a var", "echo $\\\n{{x}}"],
      ["A: line continuation joins a heredoc operator", "cat <\\\n<EOF\n{{x}}\nEOF"],
      ["A: line continuation joins a nested substitution", 'echo "$\\\n(echo "{{x}}")"'],
      ["A: line continuation hides $[", "echo $\\\n[1+{{x}}]"],
      ["A: CRLF line continuation", "echo $\\\r\n{{x}}"],
      ["A: line continuation joins $ and a quote", "echo $\\\n'\\' {{x}} '"],
      ["C: arithmetic command desyncs quotes in zsh and ksh (single)", "(( 1 ' )) ; echo ' {{x}} '"],
      ["C: arithmetic command after ||", "true || (( 1 ' )) ; echo ' {{x}} '"],
      ["C: arithmetic command (double)", '(( 1 " )) ; echo " {{x}} "'],
      ["B: nested double quotes inside a parameter expansion", 'echo "${y:-"}{{x}}"}"'],
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

    it.each([
      ["f2: comment formed when the #if body is dropped", "echo hi {{#if c}}z{{/if}}# don't\necho '{{x}}'"],
      ["f2: ANSI-C quote formed when the #if body is dropped", "echo ${{#if c}}z{{/if}}'\\' {{x}} '"],
      ["f2: heredoc formed when the #if body is dropped", "cat <{{#if c}}z{{/if}}<EOF\n{{x}}\nEOF"],
      ["f2: #if joins pieces into a var token", 'echo "{{#if a}}{{{/if}}{x}}"'],
      ["plain #if", "echo {{#if a}}yes{{/if}} {{x}}"],
    ])("rejects conditionals in command templates (%s)", (_name, tpl) => {
      const problems = shellTemplateProblems(tpl);
      expect(problems.length).toBeGreaterThan(0);
      expect(problems.some((p) => p.includes("conditionals"))).toBe(true);
    });

    it("keeps conditionals working in renderTemplate for prompts", () => {
      expect(renderTemplate("a{{#if c}}b{{/if}}", { c: true })).toBe("ab");
    });
  });
});
