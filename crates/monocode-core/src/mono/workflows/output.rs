//! Output contract for session nodes: the reply ends with one fenced JSON block the engine
//! parses.
use super::model::{OutputField, OutputFieldType};
use super::ordered::{Json, OrderedMap};
use super::template::{js_space, js_trim};

pub const OUTPUT_FENCE: &str = "workflow-output";

fn type_name(ty: OutputFieldType) -> &'static str {
    match ty {
        OutputFieldType::String => "string",
        OutputFieldType::Number => "number",
        OutputFieldType::Boolean => "boolean",
        OutputFieldType::List => "list",
        OutputFieldType::Object => "object",
    }
}

fn type_hint(ty: OutputFieldType) -> &'static str {
    match ty {
        OutputFieldType::String => "a JSON string",
        OutputFieldType::Number => "a JSON number",
        OutputFieldType::Boolean => "true or false",
        OutputFieldType::List => "a JSON array",
        OutputFieldType::Object => "a JSON object",
    }
}

fn describe_fields(fields: &OrderedMap<OutputField>) -> String {
    fields
        .iter()
        .map(|(name, field)| {
            let note = match field.description.as_deref() {
                Some(text) if !text.is_empty() => format!(": {text}"),
                _ => String::new(),
            };
            format!("- {name} ({}){note}", type_name(field.ty))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Text appended to a session prompt that tells the model how to report its result.
pub fn output_instruction(fields: &OrderedMap<OutputField>) -> String {
    if fields.is_empty() {
        return String::new();
    }
    [
        format!("When you are finished, end your final reply with exactly one fenced code block whose info string is `{OUTPUT_FENCE}`."),
        "It must contain a single JSON object with these fields:".to_string(),
        describe_fields(fields),
        "Example shape:".to_string(),
        format!("```{OUTPUT_FENCE}"),
        "{ ... }".to_string(),
        "```".to_string(),
    ]
    .join("\n")
}

/// The single follow-up message sent when the reply did not satisfy the contract.
pub fn output_reask(fields: &OrderedMap<OutputField>, problem: &str) -> String {
    format!(
        "Your last reply did not satisfy the output contract: {problem}\n\nReply again with the result as one fenced code block, nothing else required.\n{}",
        output_instruction(fields)
    )
}

/// `^ {0,3}(`{3,})([^`]*)$`: the backtick count and the info string of an opening fence.
fn open_fence(line: &str) -> Option<(usize, &str)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let info = rest.trim_start_matches('`');
    let ticks = rest.len() - info.len();
    (ticks >= 3 && !info.contains('`')).then_some((ticks, info))
}

/// `^ {0,3}`{n,}\s*$`: a line that closes a fence of `n` backticks.
fn closes_fence(line: &str, n: usize) -> bool {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return false;
    }
    let after = rest.trim_start_matches('`');
    rest.len() - after.len() >= n && after.chars().all(js_space)
}

/// The body of the last fenced block whose info string is exactly the output fence, or None.
fn last_output_block(reply: &str) -> Option<String> {
    // Split on `\r?\n`: a lone `\r` stays in the line.
    let count = reply.split('\n').count();
    let lines: Vec<&str> = reply
        .split('\n')
        .enumerate()
        .map(|(i, line)| {
            if i + 1 < count {
                line.strip_suffix('\r').unwrap_or(line)
            } else {
                line
            }
        })
        .collect();
    let mut found = None;
    let mut i = 0;
    while i < lines.len() {
        let Some((ticks, info)) = open_fence(lines[i]) else {
            i += 1;
            continue;
        };
        let is_output = js_trim(info) == OUTPUT_FENCE;
        let mut j = i + 1;
        while j < lines.len() && !closes_fence(lines[j], ticks) {
            j += 1;
        }
        // An unclosed fence runs to the end of the reply, as in Markdown.
        if is_output {
            found = Some(lines[i + 1..j].join("\n"));
        }
        i = j + 1;
    }
    found
}

fn matches_type(value: &Json, ty: OutputFieldType) -> bool {
    match ty {
        OutputFieldType::String => matches!(value, Json::String(_)),
        OutputFieldType::Number => value.as_f64().is_some(),
        OutputFieldType::Boolean => matches!(value, Json::Bool(_)),
        OutputFieldType::List => matches!(value, Json::Array(_)),
        OutputFieldType::Object => matches!(value, Json::Object(_)),
    }
}

/// Extracts and validates the `workflow-output` block from a session reply. The object keeps
/// the reply's key order, including fields the node did not declare. Err is the message for the
/// re-ask. No declared fields and no block is fine: the output is empty.
pub fn parse_workflow_output(
    reply: &str,
    fields: &OrderedMap<OutputField>,
) -> Result<OrderedMap<Json>, String> {
    let Some(body) = last_output_block(reply) else {
        if fields.is_empty() {
            return Ok(OrderedMap::new());
        }
        return Err(format!("No {OUTPUT_FENCE} block found in the reply."));
    };
    let Some(parsed) = Json::parse(&body) else {
        return Err(format!("The {OUTPUT_FENCE} block is not valid JSON."));
    };
    let Json::Object(value) = parsed else {
        return Err(format!(
            "The {OUTPUT_FENCE} block must contain a JSON object."
        ));
    };
    for (name, field) in fields.iter() {
        let ty = field.ty;
        let Some(found) = value.get(name) else {
            return Err(format!(
                "Field \"{name}\" is missing; expected {} ({}).",
                type_hint(ty),
                type_name(ty)
            ));
        };
        if !matches_type(found, ty) {
            return Err(format!(
                "Field \"{name}\" must be {} ({}).",
                type_hint(ty),
                type_name(ty)
            ));
        }
    }
    Ok(value)
}
