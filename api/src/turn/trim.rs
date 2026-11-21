//! Keeps a long turn's requests bounded: old steps leave, and old tool output and
//! arguments give way to notes.

use serde_json::{json, Value};

/// Bytes of tool output kept in the request history; older outputs are elided.
const TOOL_HISTORY_BYTES: usize = 160 * 1024;
/// Bytes of tool-call arguments kept in the request history, such as the files
/// the model wrote; older ones are elided so a long turn's requests stay bounded.
const TOOL_ARGUMENT_BYTES: usize = 160 * 1024;
/// Steps of the current turn kept in the request; a longer turn drops its oldest.
const KEPT_STEPS: usize = 150;
const STEPS_DROPPED: &str = "[Earlier steps of this turn were dropped to save context:";
const ARGUMENTS_OMITTED: &str =
    r#"{"omitted":"Earlier arguments omitted to save context. The tool result still applies."}"#;

/// Lets a turn run for any number of steps: past KEPT_STEPS, the oldest step and
/// the tool results and notes after it leave the request, and one note says so.
pub(crate) fn drop_old_steps(payload: &mut Value) {
    let Some(messages) = payload["messages"].as_array_mut() else {
        return;
    };
    let step = |m: &Value| m["role"] == "assistant" && m["tool_calls"].is_array();
    let starts: Vec<usize> = (0..messages.len())
        .filter(|&i| step(&messages[i]))
        .collect();
    if starts.len() <= KEPT_STEPS {
        return;
    }
    let (first, keep) = (starts[0], starts[starts.len() - KEPT_STEPS]);
    let note = |m: &Value| {
        m["content"]
            .as_str()
            .is_some_and(|c| c.starts_with(STEPS_DROPPED))
    };
    // An earlier note sits just before the first step; count what it already covers.
    let (from, before) = match first.checked_sub(1).filter(|&i| note(&messages[i])) {
        Some(i) => {
            let n = messages[i]["content"].as_str().unwrap_or("")[STEPS_DROPPED.len()..]
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0);
            (i, n)
        }
        None => (first, 0),
    };
    let dropped = before + starts.len() - KEPT_STEPS;
    messages.splice(
        from..keep,
        [json!({"role":"user","content":format!("{STEPS_DROPPED} {dropped} of them. Files, runs and research_status show where things stand; check them rather than relying on memory.]")})],
    );
}

/// Keeps agent requests bounded: the oldest tool outputs are replaced by a note
/// once their total passes TOOL_HISTORY_BYTES, and the oldest tool-call arguments
/// once theirs passes TOOL_ARGUMENT_BYTES.
pub(crate) fn trim_tool_history(payload: &mut Value) {
    let Some(messages) = payload["messages"].as_array_mut() else {
        return;
    };
    let mut total: usize = messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().map_or(0, str::len))
        .sum();
    for message in messages.iter_mut().filter(|m| m["role"] == "tool") {
        if total <= TOOL_HISTORY_BYTES {
            break;
        }
        let size = message["content"].as_str().map_or(0, str::len);
        if size > 200 {
            total -= size;
            message["content"] = json!(
                "[Earlier output omitted to save context. Run the tool again if you need it.]"
            );
        }
    }
    // The files and commands the model sent: in a long turn, the oldest go first.
    let mut arguments: Vec<&mut Value> = messages
        .iter_mut()
        .filter(|m| m["role"] == "assistant")
        .filter_map(|m| m["tool_calls"].as_array_mut())
        .flat_map(|calls| calls.iter_mut())
        .map(|call| &mut call["function"]["arguments"])
        .collect();
    let mut total: usize = arguments
        .iter()
        .map(|a| a.as_str().map_or(0, str::len))
        .sum();
    for argument in arguments.iter_mut() {
        if total <= TOOL_ARGUMENT_BYTES {
            break;
        }
        let size = argument.as_str().map_or(0, str::len);
        if size > ARGUMENTS_OMITTED.len() {
            total -= size;
            **argument = json!(ARGUMENTS_OMITTED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_turn_keeps_its_requests_bounded() {
        let file = "x".repeat(40 * 1024);
        let step = |n: usize| {
            [
                json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("c{n}"),"type":"function","function":{"name":"write_file","arguments":json!({"path":format!("f{n}"),"content":file}).to_string()}}]}),
                json!({"role":"tool","tool_call_id":format!("c{n}"),"content":"y".repeat(40 * 1024)}),
            ]
        };
        let mut payload = json!({"messages":(0..1000).flat_map(step).collect::<Vec<_>>()});
        drop_old_steps(&mut payload);
        trim_tool_history(&mut payload);
        let messages = payload["messages"].as_array().unwrap();
        let bytes: usize = messages.iter().map(|m| m.to_string().len()).sum();
        assert!(bytes < 400 * 1024, "{bytes} bytes after trimming");
        let first = &messages[1]["tool_calls"][0]["function"]["arguments"];
        assert_eq!(first, ARGUMENTS_OMITTED);
        assert!(serde_json::from_str::<Value>(first.as_str().unwrap()).is_ok());
        let last = &messages[messages.len() - 2]["tool_calls"][0]["function"]["arguments"];
        assert!(
            last.as_str().unwrap().contains("f999"),
            "the newest call is kept"
        );
    }

    #[test]
    fn a_turn_without_end_drops_its_oldest_steps() {
        let step = |n: usize| {
            [
                json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("c{n}"),"type":"function","function":{"name":"run","arguments":"{}"}}]}),
                json!({"role":"tool","tool_call_id":format!("c{n}"),"content":"ok"}),
            ]
        };
        let head = [
            json!({"role":"system","content":"s"}),
            json!({"role":"user","content":"go"}),
        ];
        let mut payload = json!({"messages":head.iter().cloned().chain((0..KEPT_STEPS + 10).flat_map(step)).collect::<Vec<_>>()});
        drop_old_steps(&mut payload);
        for n in KEPT_STEPS + 10..KEPT_STEPS + 15 {
            payload["messages"].as_array_mut().unwrap().extend(step(n));
            drop_old_steps(&mut payload);
        }
        let messages = payload["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3 + 2 * KEPT_STEPS);
        assert_eq!(messages[1]["content"], "go");
        assert!(messages[2]["content"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{STEPS_DROPPED} 15 of them")));
        assert_eq!(messages[3]["tool_calls"][0]["id"], "c15");
        assert_eq!(messages[4]["role"], "tool", "a step keeps its results");
    }
}
