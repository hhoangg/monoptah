//! Identity patterns and path globs. Both compile to anchored regular
//! expressions that are memoized: rescoring tests every file and identity
//! against the same few patterns.

use super::js_trim;
use crate::contributors::GitIdentity;
use regex::{Regex, RegexBuilder};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const CACHE_LIMIT: usize = 500;

#[derive(Default)]
struct Cache(Mutex<HashMap<String, Arc<Regex>>>);

impl Cache {
    fn get(&self, key: &str, make: impl FnOnce() -> Regex) -> Arc<Regex> {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(re) = map.get(key) {
            return re.clone();
        }
        if map.len() >= CACHE_LIMIT {
            map.clear();
        }
        let re = Arc::new(make());
        map.insert(key.to_string(), re.clone());
        re
    }
}

fn pattern_cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default)
}

fn glob_cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default)
}

/// A regex that matches nothing, for a pattern too large to compile.
fn never() -> Regex {
    Regex::new(r"\b\B").expect("valid regex")
}

fn compile(source: &str, case_insensitive: bool) -> Regex {
    RegexBuilder::new(source)
        .case_insensitive(case_insensitive)
        .dot_matches_new_line(true)
        .build()
        .unwrap_or_else(|_| never())
}

fn escape_char(ch: char, out: &mut String) {
    if "\\^$.*+?()[]{}|".contains(ch) {
        out.push('\\');
    }
    out.push(ch);
}

/// Identity pattern: case-insensitive, `*` is the only wildcard, must match the whole string.
pub fn match_pattern(pattern: &str, value: &str) -> bool {
    let p = js_trim(pattern);
    if p.is_empty() {
        return false;
    }
    let re = pattern_cache().get(p, || {
        let mut source = String::from("^");
        for (i, part) in p.split('*').enumerate() {
            if i > 0 {
                source.push_str(".*");
            }
            part.chars().for_each(|ch| escape_char(ch, &mut source));
        }
        source.push('$');
        compile(&source, true)
    });
    re.is_match(value)
}

/// Checks email, name, login and the `gh:<login>` key form. Empty fields are
/// skipped so a bare `*` never matches "nothing".
pub fn matches_any_identity<S: AsRef<str>>(patterns: &[S], identity: &GitIdentity) -> bool {
    let login = identity.login.as_deref().map(js_trim);
    let gh = login.filter(|l| !l.is_empty()).map(|l| format!("gh:{l}"));
    let values = [
        Some(identity.email.as_str()),
        Some(identity.name.as_str()),
        login,
        gh.as_deref(),
    ];
    let values: Vec<&str> = values
        .into_iter()
        .flatten()
        .filter(|v| !js_trim(v).is_empty())
        .collect();
    patterns.iter().any(|pattern| {
        values
            .iter()
            .any(|value| match_pattern(pattern.as_ref(), value))
    })
}

/// Git-ish glob to regex. A glob without `/` matches the basename at any depth;
/// otherwise it is anchored at the repo root. `*` stops at `/`, `**` does not.
/// Character classes like `[abc]` are not supported; brackets match literally.
pub fn glob_to_regex(glob: &str) -> Arc<Regex> {
    glob_cache().get(glob, || compile_glob(glob))
}

fn compile_glob(glob: &str) -> Regex {
    // Runs of three or more stars read as `**`.
    let mut chars: Vec<char> = Vec::new();
    let mut run = 0;
    for ch in js_trim(glob).chars() {
        if ch == '*' {
            run += 1;
            if run <= 2 {
                chars.push(ch);
            }
        } else {
            run = 0;
            chars.push(ch);
        }
    }
    let mut anchored = chars.contains(&'/');
    if chars.first() == Some(&'/') {
        chars.remove(0);
        anchored = true;
    }
    if chars.last() == Some(&'/') {
        chars.extend(['*', '*']);
    }

    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                let at_segment_start = i == 0 || chars[i - 1] == '/';
                if at_segment_start && chars.get(i + 2) == Some(&'/') {
                    out.push_str("(?:.*/)?");
                    i += 2;
                } else {
                    out.push_str(".*");
                    i += 1;
                }
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            ch => escape_char(ch, &mut out),
        }
        i += 1;
    }
    let prefix = if anchored { "" } else { "(?:.*/)?" };
    compile(&format!("^{prefix}{out}$"), false)
}

fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    match path.strip_prefix("./") {
        Some(rest) => rest.to_string(),
        None => path,
    }
}

pub fn match_glob(glob: &str, path: &str) -> bool {
    if js_trim(glob).is_empty() {
        return false;
    }
    glob_to_regex(glob).is_match(&normalize_path(path))
}

pub fn is_ignored_path<S: AsRef<str>>(globs: &[S], path: &str) -> bool {
    let normalized = normalize_path(path);
    globs.iter().any(|glob| {
        let glob = glob.as_ref();
        !js_trim(glob).is_empty() && glob_to_regex(glob).is_match(&normalized)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::config::default_config;
    use crate::contributors::GitIdentity;
    use std::sync::Arc;

    fn ident(name: &str, email: &str) -> GitIdentity {
        GitIdentity {
            name: name.into(),
            email: email.into(),
            login: None,
        }
    }

    fn ident_login(name: &str, email: &str, login: &str) -> GitIdentity {
        GitIdentity {
            login: Some(login.into()),
            ..ident(name, email)
        }
    }

    // matchPattern

    #[test]
    fn match_pattern_matches_the_whole_string_only() {
        assert!(match_pattern("abc", "abc"));
        assert!(!match_pattern("abc", "abcd"));
        assert!(!match_pattern("abc", "xabc"));
    }

    #[test]
    fn match_pattern_is_case_insensitive_on_both_sides() {
        assert!(match_pattern("Foo@Bar.COM", "foo@bar.com"));
        assert!(match_pattern("*@ANTHROPIC.com", "Noreply@Anthropic.Com"));
    }

    #[test]
    fn match_pattern_supports_star_as_any_run_of_characters_including_empty() {
        assert!(match_pattern("*claude*", "Claude Code"));
        assert!(match_pattern("*claude*", "claude"));
        assert!(match_pattern("*@ci-runner*", "build@ci-runner-12.corp"));
        assert!(match_pattern("a*c", "ac"));
        assert!(match_pattern("a*c", "abbbc"));
        assert!(!match_pattern("a*c", "abbbd"));
    }

    #[test]
    fn match_pattern_treats_every_other_character_literally() {
        assert!(match_pattern("*[bot]*", "dependabot[bot]"));
        assert!(!match_pattern("*[bot]*", "dependabotb"));
        assert!(!match_pattern("a.c", "abc"));
        assert!(match_pattern("a.c", "a.c"));
        assert!(match_pattern("a+b", "a+b"));
        assert!(!match_pattern("a?b", "axb"));
        assert!(match_pattern("(x)", "(x)"));
    }

    #[test]
    fn match_pattern_never_matches_an_empty_pattern() {
        assert!(!match_pattern("", ""));
        assert!(!match_pattern("", "abc"));
        assert!(!match_pattern("   ", "abc"));
    }

    #[test]
    fn match_pattern_trims_the_pattern() {
        assert!(match_pattern("  abc ", "abc"));
    }

    #[test]
    fn match_pattern_handles_unicode() {
        assert!(match_pattern("*Đặng*", "Nguyễn Đặng Hoàng"));
        assert!(match_pattern("hoàng", "HOÀNG"));
    }

    // matchesAnyIdentity

    fn bot() -> GitIdentity {
        ident(
            "dependabot[bot]",
            "49699333+dependabot[bot]@users.noreply.github.com",
        )
    }

    #[test]
    fn matches_any_identity_matches_on_email() {
        assert!(matches_any_identity(
            &["noreply@github.com"],
            &ident("GitHub", "noreply@github.com")
        ));
    }

    #[test]
    fn matches_any_identity_matches_on_name() {
        assert!(matches_any_identity(
            &["*claude*"],
            &ident("Claude", "x@y.z")
        ));
    }

    #[test]
    fn matches_any_identity_matches_on_login_when_present() {
        assert!(matches_any_identity(
            &["copilot-swe-agent[bot]"],
            &ident_login("", "", "copilot-swe-agent[bot]")
        ));
    }

    #[test]
    fn matches_any_identity_matches_the_gh_login_key_form() {
        assert!(matches_any_identity(
            &["gh:alice"],
            &ident_login("", "", "Alice")
        ));
    }

    #[test]
    fn matches_any_identity_matches_when_any_pattern_matches() {
        assert!(matches_any_identity(&["nope", "*[bot]*"], &bot()));
        assert!(!matches_any_identity(&["nope", "other"], &bot()));
    }

    #[test]
    fn matches_any_identity_returns_false_for_no_patterns() {
        let none: [&str; 0] = [];
        assert!(!matches_any_identity(&none, &bot()));
    }

    #[test]
    fn matches_any_identity_does_not_let_a_wildcard_match_an_empty_email_or_name() {
        assert!(!matches_any_identity(&["*"], &ident("", "")));
    }

    #[test]
    fn matches_any_identity_ignores_blank_patterns() {
        assert!(!matches_any_identity(&["", "  "], &bot()));
    }

    // matchGlob

    #[test]
    fn match_glob_matches_a_slashless_glob_against_the_basename_at_any_depth() {
        assert!(match_glob("*.lock", "yarn.lock"));
        assert!(match_glob("*.lock", "a/b/yarn.lock"));
        assert!(!match_glob("*.lock", "yarn.lock.bak"));
        assert!(match_glob("package-lock.json", "web/package-lock.json"));
        assert!(!match_glob("package-lock.json", "web/my-package-lock.json"));
        assert!(match_glob("Cargo.lock", "src-tauri/Cargo.lock"));
    }

    #[test]
    fn match_glob_does_not_match_a_slashless_glob_against_directory_names() {
        assert!(!match_glob("*.lock", "a.lock/readme.md"));
    }

    #[test]
    fn match_glob_anchors_a_glob_that_contains_a_slash_at_the_root() {
        assert!(match_glob("dist/**", "dist/a.js"));
        assert!(match_glob("dist/**", "dist/a/b/c.js"));
        assert!(!match_glob("dist/**", "packages/dist/a.js"));
        assert!(!match_glob("dist/**", "distant/a.js"));
    }

    #[test]
    fn match_glob_does_not_match_the_directory_name_itself_for_dir_star_star() {
        assert!(!match_glob("dist/**", "dist"));
    }

    #[test]
    fn match_glob_treats_star_as_any_chars_except_slash() {
        assert!(match_glob("src/*.ts", "src/a.ts"));
        assert!(!match_glob("src/*.ts", "src/a/b.ts"));
        assert!(match_glob("src/a*", "src/abc"));
    }

    #[test]
    fn match_glob_treats_double_star_as_any_chars_including_slash() {
        assert!(match_glob("src/**/x.ts", "src/x.ts"));
        assert!(match_glob("src/**/x.ts", "src/a/x.ts"));
        assert!(match_glob("src/**/x.ts", "src/a/b/x.ts"));
        assert!(!match_glob("src/**/x.ts", "src/a/b/y.ts"));
        assert!(!match_glob("src/**/x.ts", "other/a/x.ts"));
    }

    #[test]
    fn match_glob_lets_double_star_slash_x_match_x_at_any_depth_including_the_root() {
        assert!(match_glob("**/x", "x"));
        assert!(match_glob("**/x", "a/x"));
        assert!(match_glob("**/x", "a/b/x"));
        assert!(!match_glob("**/x", "a/xx"));
        assert!(match_glob("**/*.snap", "a.snap"));
        assert!(match_glob("**/*.snap", "src/__s__/a.snap"));
        assert!(match_glob("**/*.min.js", "public/app.min.js"));
        assert!(match_glob("**/*.min.js", "app.min.js"));
        assert!(!match_glob("**/*.min.js", "app.min.jsx"));
    }

    #[test]
    fn match_glob_supports_double_star_dir_double_star_at_any_depth() {
        assert!(match_glob("**/generated/**", "generated/a.ts"));
        assert!(match_glob("**/generated/**", "src/generated/a/b.ts"));
        assert!(!match_glob("**/generated/**", "src/notgenerated/a.ts"));
    }

    #[test]
    fn match_glob_supports_star_inside_a_slashless_glob_with_several_wildcards() {
        assert!(match_glob("*.generated.*", "api.generated.ts"));
        assert!(match_glob("*.generated.*", "src/api.generated.ts"));
        assert!(!match_glob("*.generated.*", "api.generated"));
    }

    #[test]
    fn match_glob_supports_question_mark_as_a_single_non_slash_character() {
        assert!(match_glob("a?c.ts", "abc.ts"));
        assert!(!match_glob("a?c.ts", "ac.ts"));
        assert!(!match_glob("a?c.ts", "abbc.ts"));
        assert!(!match_glob("src/a?c", "src/a/c"));
    }

    #[test]
    fn match_glob_escapes_regex_metacharacters() {
        assert!(match_glob("a.b", "a.b"));
        assert!(!match_glob("a.b", "axb"));
        assert!(match_glob("(a)+.ts", "(a)+.ts"));
        assert!(match_glob("a[1].ts", "a[1].ts"));
        assert!(!match_glob("a[1].ts", "a1.ts"));
        assert!(match_glob("a$b^c|d", "a$b^c|d"));
    }

    #[test]
    fn match_glob_strips_a_leading_slash_and_anchors() {
        assert!(match_glob("/build/**", "build/a.js"));
        assert!(!match_glob("/build/**", "x/build/a.js"));
        assert!(match_glob("/README.md", "README.md"));
        assert!(!match_glob("/README.md", "docs/README.md"));
    }

    #[test]
    fn match_glob_treats_a_trailing_slash_as_a_whole_directory() {
        assert!(match_glob("vendor/", "vendor/a/b.go"));
        assert!(!match_glob("vendor/", "other/vendor/a.go"));
    }

    #[test]
    fn match_glob_matches_everything_for_a_lone_double_star() {
        assert!(match_glob("**", "a"));
        assert!(match_glob("**", "a/b/c"));
    }

    #[test]
    fn match_glob_normalizes_backslashes_and_a_leading_dot_slash_in_the_path() {
        assert!(match_glob("dist/**", "dist\\a\\b.js"));
        assert!(match_glob("dist/**", "./dist/a.js"));
    }

    #[test]
    fn match_glob_is_case_sensitive_like_git() {
        assert!(!match_glob("*.lock", "YARN.LOCK"));
    }

    #[test]
    fn match_glob_never_matches_a_blank_glob() {
        assert!(!match_glob("", "a"));
        assert!(!match_glob("  ", "a"));
    }

    #[test]
    fn match_glob_handles_unicode_paths() {
        assert!(match_glob("docs/**", "docs/tài-liệu/hướng-dẫn.md"));
        assert!(match_glob("*.md", "ghi-chú.md"));
    }

    // globToRegExp

    #[test]
    fn glob_to_regex_returns_an_anchored_regex() {
        let re = glob_to_regex("*.lock");
        assert!(re.is_match("a.lock"));
        assert!(!re.is_match("a.lock.x"));
    }

    #[test]
    fn glob_to_regex_returns_the_same_compiled_regex_for_the_same_glob() {
        assert!(Arc::ptr_eq(
            &glob_to_regex("*.lock"),
            &glob_to_regex("*.lock")
        ));
        assert!(!Arc::ptr_eq(
            &glob_to_regex("*.lock"),
            &glob_to_regex("*.snap")
        ));
    }

    #[test]
    fn glob_to_regex_is_stateless_across_repeated_test_calls() {
        let re = glob_to_regex("*.lock");
        assert!(re.is_match("a.lock"));
        assert!(re.is_match("a.lock"));
    }

    #[test]
    fn glob_to_regex_collapses_runs_of_three_or_more_stars_to_double_star() {
        assert!(glob_to_regex("a/***/b").is_match("a/x/y/b"));
    }

    // isIgnoredPath

    const GLOBS: [&str; 3] = ["*.lock", "dist/**", "**/*.snap"];

    #[test]
    fn is_ignored_path_is_true_when_any_glob_matches() {
        assert!(is_ignored_path(&GLOBS, "Cargo.lock"));
        assert!(is_ignored_path(&GLOBS, "dist/a.js"));
        assert!(is_ignored_path(&GLOBS, "src/__snapshots__/a.snap"));
    }

    #[test]
    fn is_ignored_path_is_false_when_none_match() {
        assert!(!is_ignored_path(&GLOBS, "src/a.ts"));
    }

    #[test]
    fn is_ignored_path_is_false_for_no_globs() {
        let none: [&str; 0] = [];
        assert!(!is_ignored_path(&none, "src/a.ts"));
    }

    #[test]
    fn is_ignored_path_skips_blank_globs() {
        assert!(!is_ignored_path(&["", " "], "src/a.ts"));
    }

    // not in the TS suite: the cache and unusual input must never panic

    #[test]
    fn caches_survive_overflow_and_pathological_input() {
        for i in 0..1200 {
            assert!(match_pattern(&format!("p{i}*"), &format!("p{i}x")));
            assert!(match_glob(&format!("d{i}/**"), &format!("d{i}/a")));
        }
        let long = "a/".repeat(5000);
        let _ = match_glob(&long, "a/a");
        assert!(!is_ignored_path(&default_config().ignore, "src/app.ts"));
    }
}
