//! Pieces of an agent turn that stand apart from run_generation: reading tool calls
//! out of what the model sends, keeping the turn's requests bounded, and answering
//! its tool calls.

pub(crate) mod parse;
pub(crate) mod trim;

use serde_json::{json, Value};

/// What the model is told when it calls a tool this turn does not offer.
pub(crate) fn no_such_tool(name: &str) -> String {
    format!("There is no tool named {name}.")
}

/// What the model is told when its reply already ends with a question or card.
pub(crate) const ONE_AT_A_TIME: &str =
    "Ask for one thing at a time: your reply already ends with a question.";

/// Answers a tool call in the request: what the tool returned, or why it did not run.
pub(crate) fn tool_result(payload: &mut Value, call_id: &str, content: impl Into<Value>) {
    if let Some(messages) = payload["messages"].as_array_mut() {
        messages.push(json!({"role":"tool","tool_call_id":call_id,"content":content.into()}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_result_answers_its_call() {
        let mut payload = json!({"messages":[{"role":"user","content":"Hi"}]});
        tool_result(&mut payload, "call_1", no_such_tool("dance"));
        tool_result(&mut payload, "call_2", ONE_AT_A_TIME);
        assert_eq!(
            payload["messages"][1],
            json!({"role":"tool","tool_call_id":"call_1","content":"There is no tool named dance."})
        );
        assert_eq!(payload["messages"][2]["content"], ONE_AT_A_TIME);
    }
}
