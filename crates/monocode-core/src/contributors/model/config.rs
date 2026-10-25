//! `.monoptah/contributors.json`: defaults, tolerant reading and the
//! byte-stable writer. The Tauri app reads and writes the same file, so the
//! key order, indent and number format match `config.ts` exactly.

use super::js_trim;
use super::types::{ContributorsConfig, Person, Weights};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::LazyLock;

pub const CONFIG_PATH: &str = ".monoptah/contributors.json";

const DEFAULT_IGNORE: [&str; 16] = [
    "*.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "Cargo.lock",
    "go.sum",
    "dist/**",
    "build/**",
    "vendor/**",
    "**/dist/**",
    "**/build/**",
    "**/vendor/**",
    "**/*.snap",
    "**/*.min.js",
    "**/*.generated.*",
    "**/generated/**",
];

// Generic bots are excluded outright; AI agents are kept (see agentIdentities)
// and an identity matching both counts as an agent.
const DEFAULT_EXCLUDE: [&str; 15] = [
    "*[bot]*",
    "dependabot[bot]",
    "renovate[bot]",
    "github-actions[bot]",
    "noreply@github.com",
    "*@ci-runner*",
    "coderabbitai*",
    "copilot-pull-request-reviewer*",
    "*codecov*",
    "*sonarcloud*",
    "gemini-code-assist*",
    "sourcery-ai*",
    "github-advanced-security*",
    "vercel*",
    "netlify*",
];

// Whole-string matches, so a human named Claude Dupont is not an agent.
const DEFAULT_AGENTS: [&str; 18] = [
    "*@anthropic.com",
    "claude",
    "codex",
    "copilot",
    "devin",
    "gemini",
    "cursor agent",
    "cursoragent@cursor.com",
    "*@cursor.com",
    "copilot@github.com",
    "*@openai.com",
    "*@cognition.ai",
    "copilot-swe-agent[bot]",
    "*copilot-swe-agent*",
    "devin-ai-integration[bot]",
    "*devin-ai-integration*",
    "claude[bot]",
    "*claude[bot]*",
];

fn owned(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// A fresh copy of the defaults; edit it freely.
pub fn default_config() -> ContributorsConfig {
    ContributorsConfig {
        version: 1,
        season: "monthly".into(),
        weights: default_weights(),
        ignore: owned(&DEFAULT_IGNORE),
        exclude_identities: owned(&DEFAULT_EXCLUDE),
        agent_identities: owned(&DEFAULT_AGENTS),
        people: vec![],
        dismissed_merges: vec![],
    }
}

fn default_weights() -> Weights {
    Weights {
        delivery: 1.0,
        review: 1.0,
        care: 1.0,
        consistency: 1.0,
        breadth: 1.0,
    }
}

/// The defaults, for read-only use.
pub static DEFAULT_CONFIG: LazyLock<ContributorsConfig> = LazyLock::new(default_config);

impl Default for ContributorsConfig {
    fn default() -> Self {
        default_config()
    }
}

fn unique(values: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values.filter(|v| seen.insert(v.clone())).collect()
}

/// Non-string and blank entries are dropped; `None` when the field is not an array.
fn string_list(value: Option<&Value>, lowercase: bool) -> Option<Vec<String>> {
    let items = value?.as_array()?;
    Some(unique(
        items
            .iter()
            .filter_map(Value::as_str)
            .map(|v| {
                let v = js_trim(v);
                if lowercase {
                    v.to_lowercase()
                } else {
                    v.to_string()
                }
            })
            .filter(|v| !v.is_empty()),
    ))
}

fn parse_weights(value: Option<&Value>) -> Weights {
    let mut out = default_weights();
    let Some(map) = value.and_then(Value::as_object) else {
        return out;
    };
    let read = |key: &str, slot: &mut f64| {
        if let Some(w) = map
            .get(key)
            .and_then(Value::as_f64)
            .filter(|w| w.is_finite())
        {
            *slot = w.clamp(0.0, 5.0);
        }
    };
    read("delivery", &mut out.delivery);
    read("review", &mut out.review);
    read("care", &mut out.care);
    read("consistency", &mut out.consistency);
    read("breadth", &mut out.breadth);
    out
}

fn parse_people(value: Option<&Value>) -> Vec<Person> {
    let Some(entries) = value.and_then(Value::as_array) else {
        return vec![];
    };
    let mut seen = HashSet::new();
    let mut people = Vec::new();
    for entry in entries {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let id = entry.get("id").and_then(Value::as_str).map_or("", js_trim);
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .map(js_trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(id);
        people.push(Person {
            id: id.to_string(),
            name: name.to_string(),
            identities: string_list(entry.get("identities"), true).unwrap_or_default(),
        });
    }
    people
}

fn describe_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Array(_) => "an array",
        Value::String(_) => "a string",
        Value::Number(_) => "a number",
        Value::Bool(_) => "a boolean",
        Value::Object(_) => "an object",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigResult {
    pub config: ContributorsConfig,
    /// Set when the file exists but cannot be used as a whole (broken JSON, or
    /// not an object): callers must refuse to save over it.
    pub error: Option<String>,
}

/// Never panics; every field falls back to its default on bad input. `error` is
/// set when the file exists but is not usable as a whole, so callers can refuse
/// to save over it. The returned config is still the defaults, for display.
pub fn parse_config_result(text: Option<&str>) -> ConfigResult {
    let mut raw = Value::Null;
    let mut error = None;
    if let Some(text) = text.filter(|t| !js_trim(t).is_empty()) {
        match serde_json::from_str::<Value>(text) {
            Ok(value) if value.is_object() => raw = value,
            Ok(value) => {
                error = Some(format!(
                    "Expected a JSON object at the top level, found {}",
                    describe_type(&value)
                ));
            }
            Err(e) => error = Some(format!("Invalid JSON: {e}")),
        }
    }
    let src = raw.as_object();
    let field = |key: &str| src.and_then(|m| m.get(key));
    let defaults = default_config();
    let config = ContributorsConfig {
        version: 1,
        season: "monthly".into(),
        weights: parse_weights(field("weights")),
        ignore: string_list(field("ignore"), false).unwrap_or(defaults.ignore),
        exclude_identities: string_list(field("excludeIdentities"), false)
            .unwrap_or(defaults.exclude_identities),
        agent_identities: string_list(field("agentIdentities"), false)
            .unwrap_or(defaults.agent_identities),
        people: parse_people(field("people")),
        dismissed_merges: string_list(field("dismissedMerges"), false).unwrap_or_default(),
    };
    ConfigResult { config, error }
}

/// Never panics; see `parse_config_result` to learn whether the file was valid.
pub fn parse_config(text: Option<&str>) -> ContributorsConfig {
    parse_config_result(text).config
}

/// Stable key order, 2-space JSON, trailing newline, so diffs stay small.
pub fn serialize_config(config: &ContributorsConfig) -> String {
    let mut text = serde_json::to_string_pretty(config).expect("config serializes");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::patterns::{is_ignored_path, matches_any_identity};
    use crate::contributors::GitIdentity;
    use serde_json::{json, Value};

    fn ident(name: &str, email: &str) -> GitIdentity {
        GitIdentity {
            name: name.into(),
            email: email.into(),
            login: None,
        }
    }

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn parse(value: Value) -> ContributorsConfig {
        parse_config(Some(&value.to_string()))
    }

    #[test]
    fn config_path_points_at_the_fork_local_config_file() {
        assert_eq!(CONFIG_PATH, ".monoptah/contributors.json");
    }

    // DEFAULT_CONFIG

    #[test]
    fn default_config_has_the_documented_shape() {
        let d = default_config();
        assert_eq!(d.version, 1);
        assert_eq!(d.season, "monthly");
        assert_eq!(
            d.weights,
            Weights {
                delivery: 1.0,
                review: 1.0,
                care: 1.0,
                consistency: 1.0,
                breadth: 1.0
            }
        );
        assert_eq!(d.people, vec![]);
        assert_eq!(d.dismissed_merges, Vec::<String>::new());
        assert_eq!(*DEFAULT_CONFIG, d);
    }

    #[test]
    fn default_config_ignores_lockfiles_build_output_and_generated_files() {
        let d = default_config();
        for path in [
            "yarn.lock",
            "web/package-lock.json",
            "pnpm-lock.yaml",
            "src-tauri/Cargo.lock",
            "go.sum",
            "dist/a.js",
            "build/a.js",
            "vendor/x/y.go",
            "src/__snapshots__/a.test.ts.snap",
            "public/app.min.js",
            "src/api.generated.ts",
            "src/generated/x.ts",
        ] {
            assert!(is_ignored_path(&d.ignore, path), "{path}");
        }
        assert!(!is_ignored_path(&d.ignore, "src/app.ts"));
    }

    #[test]
    fn default_config_lists_exactly_the_narrowed_agent_identities() {
        assert_eq!(
            default_config().agent_identities,
            strs(&[
                "*@anthropic.com",
                "claude",
                "codex",
                "copilot",
                "devin",
                "gemini",
                "cursor agent",
                "cursoragent@cursor.com",
                "*@cursor.com",
                "copilot@github.com",
                "*@openai.com",
                "*@cognition.ai",
                "copilot-swe-agent[bot]",
                "*copilot-swe-agent*",
                "devin-ai-integration[bot]",
                "*devin-ai-integration*",
                "claude[bot]",
                "*claude[bot]*",
            ])
        );
    }

    #[test]
    fn default_config_classifies_the_claude_github_app_as_an_agent_not_excluded() {
        let d = default_config();
        let app = GitIdentity {
            name: "claude[bot]".into(),
            email: "209825114+claude[bot]@users.noreply.github.com".into(),
            login: Some("claude[bot]".into()),
        };
        assert!(matches_any_identity(&d.agent_identities, &app));
        assert!(matches_any_identity(
            &d.agent_identities,
            &ident("", &app.email)
        ));
    }

    #[test]
    fn default_config_ignores_build_output_at_any_depth_in_monorepos() {
        let d = default_config();
        for path in [
            "packages/app/dist/x.js",
            "apps/web/build/x.js",
            "services/api/vendor/x.go",
        ] {
            assert!(is_ignored_path(&d.ignore, path), "{path}");
        }
        for g in ["**/dist/**", "**/build/**", "**/vendor/**", "dist/**"] {
            assert!(d.ignore.iter().any(|x| x == g), "{g}");
        }
    }

    #[test]
    fn default_config_keeps_generic_bots_out_of_the_agent_list_and_in_the_exclude_list() {
        let d = default_config();
        for p in [
            "dependabot[bot]",
            "renovate[bot]",
            "github-actions[bot]",
            "noreply@github.com",
            "*@ci-runner*",
        ] {
            assert!(d.exclude_identities.iter().any(|x| x == p), "{p}");
            assert!(!d.agent_identities.iter().any(|x| x == p), "{p}");
        }
        for name in ["dependabot[bot]", "renovate[bot]", "github-actions[bot]"] {
            let who = ident(name, &format!("1+{name}@users.noreply.github.com"));
            assert!(!matches_any_identity(&d.agent_identities, &who));
            assert!(matches_any_identity(&d.exclude_identities, &who));
        }
    }

    // DEFAULT_CONFIG service accounts

    #[test]
    fn service_accounts_are_excluded_but_kept_out_of_the_agent_list() {
        let d = default_config();
        for name in [
            "coderabbitai[bot]",
            "coderabbitai",
            "copilot-pull-request-reviewer",
            "copilot-pull-request-reviewer[bot]",
            "codecov-commenter",
            "codecov[bot]",
            "sonarcloud[bot]",
            "SonarCloud",
            "gemini-code-assist[bot]",
            "sourcery-ai[bot]",
            "github-advanced-security[bot]",
            "vercel[bot]",
            "vercel",
            "netlify[bot]",
            "netlify",
        ] {
            let who = GitIdentity {
                name: name.into(),
                email: String::new(),
                login: Some(name.into()),
            };
            assert!(matches_any_identity(&d.exclude_identities, &who), "{name}");
            assert!(!matches_any_identity(&d.agent_identities, &who), "{name}");
        }
    }

    #[test]
    fn service_account_patterns_are_added_without_dropping_the_earlier_defaults() {
        let d = default_config();
        for p in [
            "*[bot]*",
            "dependabot[bot]",
            "renovate[bot]",
            "github-actions[bot]",
            "noreply@github.com",
            "*@ci-runner*",
            "coderabbitai*",
            "copilot-pull-request-reviewer*",
            "*codecov*",
            "*sonarcloud*",
            "gemini-code-assist*",
            "sourcery-ai*",
            "github-advanced-security*",
            "vercel*",
            "netlify*",
        ] {
            assert!(d.exclude_identities.iter().any(|x| x == p), "{p}");
        }
    }

    #[test]
    fn service_account_patterns_do_not_exclude_ordinary_people() {
        let d = default_config();
        for name in ["Alice Smith", "Vera Cruz", "Nate Lify"] {
            assert!(
                !matches_any_identity(&d.exclude_identities, &ident(name, "x@acme.com")),
                "{name}"
            );
        }
    }

    // parseConfig

    #[test]
    fn parse_config_returns_defaults_for_missing_input() {
        assert_eq!(parse_config(None), default_config());
        assert_eq!(parse_config(Some("")), default_config());
        assert_eq!(parse_config(Some("   \n")), default_config());
    }

    #[test]
    fn parse_config_returns_defaults_for_invalid_json() {
        assert_eq!(parse_config(Some("{not json")), default_config());
        assert_eq!(parse_config(Some("<<<<<<< HEAD")), default_config());
    }

    #[test]
    fn parse_config_returns_defaults_for_non_object_json() {
        for text in ["[]", "42", "\"x\"", "null", "true"] {
            assert_eq!(parse_config(Some(text)), default_config(), "{text}");
        }
    }

    #[test]
    fn parse_config_never_returns_shared_state_with_the_defaults() {
        let mut a = parse_config(None);
        a.ignore.push("mutated".into());
        a.weights.delivery = 4.0;
        let d = default_config();
        assert!(!d.ignore.iter().any(|x| x == "mutated"));
        assert_eq!(d.weights.delivery, 1.0);
        assert!(!DEFAULT_CONFIG.ignore.iter().any(|x| x == "mutated"));
        assert!(!parse_config(None).ignore.iter().any(|x| x == "mutated"));
    }

    #[test]
    fn parse_config_falls_back_field_by_field() {
        let cfg = parse(json!({
            "weights": { "delivery": 2 },
            "ignore": "oops",
            "excludeIdentities": ["me@x.com"],
            "people": "nope",
            "dismissedMerges": 7,
        }));
        let d = default_config();
        assert_eq!(
            cfg.weights,
            Weights {
                delivery: 2.0,
                ..d.weights.clone()
            }
        );
        assert_eq!(cfg.ignore, d.ignore);
        assert_eq!(cfg.exclude_identities, strs(&["me@x.com"]));
        assert_eq!(cfg.agent_identities, d.agent_identities);
        assert_eq!(cfg.people, vec![]);
        assert_eq!(cfg.dismissed_merges, Vec::<String>::new());
    }

    #[test]
    fn parse_config_clamps_weights_to_0_5() {
        let cfg = parse(json!({
            "weights": { "delivery": -3, "review": 9, "care": 0, "consistency": 2.5, "breadth": 5 }
        }));
        assert_eq!(
            cfg.weights,
            Weights {
                delivery: 0.0,
                review: 5.0,
                care: 0.0,
                consistency: 2.5,
                breadth: 5.0
            }
        );
    }

    #[test]
    fn parse_config_falls_back_to_1_for_non_numeric_or_non_finite_weights() {
        // 1e999 is not representable: the reader reports the file as invalid,
        // the JS model would read it as Infinity; both end on the defaults.
        let cfg = parse_config(Some(
            r#"{"weights":{"delivery":"3","review":null,"care":true,"consistency":[],"breadth":1e999}}"#,
        ));
        assert_eq!(cfg.weights, default_config().weights);
        let cfg = parse_config(Some(
            r#"{"weights":{"delivery":"3","review":null,"care":true,"consistency":[],"breadth":"x"}}"#,
        ));
        assert_eq!(cfg.weights, default_config().weights);
    }

    #[test]
    fn parse_config_ignores_a_weights_field_that_is_not_an_object() {
        let d = default_config();
        assert_eq!(parse_config(Some(r#"{"weights":3}"#)).weights, d.weights);
        assert_eq!(
            parse_config(Some(r#"{"weights":[1,2]}"#)).weights,
            d.weights
        );
    }

    #[test]
    fn parse_config_drops_non_string_entries_trims_drops_blanks_and_dedupes() {
        let cfg = parse(json!({
            "ignore": ["*.lock", 5, null, " *.lock ", "", "  ", "dist/**", {}],
            "excludeIdentities": ["a@x.com", "a@x.com", 1],
            "agentIdentities": [],
            "dismissedMerges": ["x:same-name", "x:same-name", false],
        }));
        assert_eq!(cfg.ignore, strs(&["*.lock", "dist/**"]));
        assert_eq!(cfg.exclude_identities, strs(&["a@x.com"]));
        assert_eq!(cfg.agent_identities, Vec::<String>::new());
        assert_eq!(cfg.dismissed_merges, strs(&["x:same-name"]));
    }

    #[test]
    fn parse_config_keeps_an_explicitly_emptied_list_instead_of_restoring_defaults() {
        assert_eq!(
            parse_config(Some(r#"{"ignore":[]}"#)).ignore,
            Vec::<String>::new()
        );
    }

    #[test]
    fn parse_config_parses_people_and_normalizes_their_identities() {
        let cfg = parse(json!({
            "people": [
                {
                    "id": "p1",
                    "name": "Alice",
                    "identities": ["Alice@Example.COM", "gh:Alice", " alice@example.com ", 3],
                },
                { "id": "p2", "name": "Bob" },
            ]
        }));
        assert_eq!(
            cfg.people,
            vec![
                Person {
                    id: "p1".into(),
                    name: "Alice".into(),
                    identities: strs(&["alice@example.com", "gh:alice"]),
                },
                Person {
                    id: "p2".into(),
                    name: "Bob".into(),
                    identities: vec![],
                },
            ]
        );
    }

    #[test]
    fn parse_config_drops_malformed_people_and_duplicate_ids() {
        let cfg = parse(json!({
            "people": [
                null,
                "x",
                [],
                { "name": "No id" },
                { "id": "", "name": "Empty id" },
                { "id": 5, "name": "Numeric id" },
                { "id": "dup", "name": "First", "identities": ["a@x.com"] },
                { "id": "dup", "name": "Second", "identities": ["b@x.com"] },
                { "id": "noname", "identities": ["c@x.com"] },
            ]
        }));
        let ids: Vec<&str> = cfg.people.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["dup", "noname"]);
        assert_eq!(cfg.people[0].name, "First");
        assert_eq!(cfg.people[1].name, "noname");
    }

    #[test]
    fn parse_config_forces_version_1_and_monthly_season() {
        let cfg = parse_config(Some(r#"{"version":9,"season":"weekly"}"#));
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.season, "monthly");
    }

    #[test]
    fn parse_config_handles_unicode_names() {
        let cfg = parse(json!({
            "people": [{ "id": "p", "name": "Nguyễn Văn Đạt", "identities": ["Đạt"] }]
        }));
        assert_eq!(cfg.people[0].name, "Nguyễn Văn Đạt");
        assert_eq!(cfg.people[0].identities, strs(&["đạt"]));
    }

    // serializeConfig

    #[test]
    fn serialize_config_emits_2_space_json_with_a_trailing_newline() {
        let text = serialize_config(&default_config());
        assert!(text.ends_with('\n'));
        assert!(!text.ends_with("\n\n"));
        assert!(text.contains("\n  \"version\": 1,"));
    }

    fn keys(value: &Value) -> Vec<String> {
        // preserve_order is off, so read the key order from the text
        value.as_object().unwrap().keys().cloned().collect()
    }

    #[test]
    fn serialize_config_uses_a_stable_key_order() {
        let text = serialize_config(&default_config());
        let mut at = 0;
        for key in [
            "version",
            "season",
            "weights",
            "ignore",
            "excludeIdentities",
            "agentIdentities",
            "people",
            "dismissedMerges",
        ] {
            let found = text[at..]
                .find(&format!("\n  \"{key}\":"))
                .unwrap_or_else(|| {
                    panic!("{key} missing or out of order in {text}");
                });
            at += found + 1;
        }
        let weights = text.find("\"weights\"").unwrap();
        let mut at = weights;
        for key in ["delivery", "review", "care", "consistency", "breadth"] {
            let found = text[at..].find(&format!("\n    \"{key}\":")).unwrap();
            at += found + 1;
        }
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(keys(&parsed).len(), 8);
    }

    #[test]
    fn serialize_config_orders_keys_the_same_regardless_of_input_key_order() {
        let shuffled = parse_config(Some(
            r#"{"people":[{"identities":["a@x.com"],"name":"A","id":"a"}],"weights":{"breadth":2,"delivery":3}}"#,
        ));
        let text = serialize_config(&shuffled);
        assert!(text.starts_with("{\n  \"version\": 1,"));
        let id = text.find("\"id\": \"a\"").unwrap();
        let name = text.find("\"name\": \"A\"").unwrap();
        let identities = text.rfind("\"identities\"").unwrap();
        assert!(id < name && name < identities);
    }

    #[test]
    fn serialize_config_round_trips_through_parse_config() {
        let cfg = parse(json!({
            "weights": { "delivery": 2, "review": 0.5 },
            "ignore": ["a/**"],
            "people": [{ "id": "p", "name": "P", "identities": ["p@x.com", "gh:p"] }],
            "dismissedMerges": ["a|b:same-name"],
        }));
        let text = serialize_config(&cfg);
        assert_eq!(parse_config(Some(&text)), cfg);
        assert_eq!(serialize_config(&parse_config(Some(&text))), text);
    }

    #[test]
    fn serialize_config_is_idempotent_for_the_defaults() {
        let d = default_config();
        assert_eq!(parse_config(Some(&serialize_config(&d))), d);
    }

    #[test]
    fn serialize_config_prints_whole_weights_like_js() {
        // JSON.stringify prints 1, not 1.0; both apps write the same file.
        let mut cfg = default_config();
        cfg.weights.review = 0.5;
        cfg.weights.care = 2.0;
        let text = serialize_config(&cfg);
        assert!(text.contains("\n    \"delivery\": 1,"), "{text}");
        assert!(text.contains("\n    \"review\": 0.5,"), "{text}");
        assert!(text.contains("\n    \"care\": 2,"), "{text}");
    }

    #[test]
    fn serialize_config_matches_the_js_output_byte_for_byte() {
        // Produced by `JSON.stringify(ordered, null, 2) + "\n"` in src/features/contributors/model/config.ts.
        let cfg = ContributorsConfig {
            version: 1,
            season: "monthly".into(),
            weights: Weights {
                delivery: 2.0,
                review: 0.5,
                care: 1.0,
                consistency: 0.0,
                breadth: 5.0,
            },
            ignore: strs(&["*.lock", "dist/**"]),
            exclude_identities: vec![],
            agent_identities: strs(&["claude"]),
            people: vec![Person {
                id: "p1".into(),
                name: "Nguyễn \"Đạt\"\n".into(),
                identities: strs(&["a@x.com", "gh:a"]),
            }],
            dismissed_merges: vec![],
        };
        let expected = "{\n  \"version\": 1,\n  \"season\": \"monthly\",\n  \"weights\": {\n    \"delivery\": 2,\n    \"review\": 0.5,\n    \"care\": 1,\n    \"consistency\": 0,\n    \"breadth\": 5\n  },\n  \"ignore\": [\n    \"*.lock\",\n    \"dist/**\"\n  ],\n  \"excludeIdentities\": [],\n  \"agentIdentities\": [\n    \"claude\"\n  ],\n  \"people\": [\n    {\n      \"id\": \"p1\",\n      \"name\": \"Nguyễn \\\"Đạt\\\"\\n\",\n      \"identities\": [\n        \"a@x.com\",\n        \"gh:a\"\n      ]\n    }\n  ],\n  \"dismissedMerges\": []\n}\n";
        assert_eq!(serialize_config(&cfg), expected);
    }

    // parseConfigResult

    #[test]
    fn parse_config_result_reports_no_error_for_a_missing_empty_or_blank_file() {
        for text in [None, Some(""), Some("  \n")] {
            let result = parse_config_result(text);
            assert_eq!(result.error, None);
            assert_eq!(result.config, default_config());
        }
    }

    #[test]
    fn parse_config_result_reports_no_error_for_a_valid_object_even_with_unknown_or_bad_fields() {
        let result = parse_config_result(Some(
            &json!({ "weights": { "delivery": "x" }, "extra": 1 }).to_string(),
        ));
        assert_eq!(result.error, None);
        assert_eq!(result.config.weights.delivery, 1.0);
    }

    #[test]
    fn parse_config_result_reports_broken_json_with_the_parsers_reason_and_falls_back_to_defaults()
    {
        let result = parse_config_result(Some(r#"{ "people": [ ],, }"#));
        let error = result.error.unwrap();
        assert!(error.starts_with("Invalid JSON: ") && error.len() > "Invalid JSON: ".len());
        assert_eq!(result.config, default_config());
    }

    #[test]
    fn parse_config_result_reports_merge_conflict_markers() {
        let result = parse_config_result(Some("<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>>"));
        assert!(result.error.unwrap().starts_with("Invalid JSON"));
    }

    #[test]
    fn parse_config_result_reports_a_wrong_top_level_type() {
        for (text, what) in [
            ("[]", "an array"),
            ("\"text\"", "a string"),
            ("42", "a number"),
            ("true", "a boolean"),
            ("null", "null"),
        ] {
            let result = parse_config_result(Some(text));
            assert_eq!(
                result.error,
                Some(format!(
                    "Expected a JSON object at the top level, found {what}"
                )),
                "{text}"
            );
            assert_eq!(result.config, default_config(), "{text}");
        }
    }

    #[test]
    fn parse_config_stays_a_never_panicking_wrapper_over_the_same_config() {
        assert_eq!(parse_config(Some("{ nope")), default_config());
        assert_eq!(parse_config(Some("[1]")), default_config());
        let text = json!({ "ignore": ["a/**"] }).to_string();
        assert_eq!(
            parse_config(Some(&text)),
            parse_config_result(Some(&text)).config
        );
    }
}
