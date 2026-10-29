//! Questions the model asks the person with a few choices each, shown as a card at
//! the end of the reply. The reply ends at the card; the person's answers arrive as
//! their next message, so an answer still works hours later or after a restart.

use serde_json::{json, Value};

pub const TOOL: &str = "ask_user";
const MAX_QUESTIONS: usize = 4;
const MAX_OPTIONS: usize = 6;

pub fn definition() -> Value {
    json!({"type":"function","function":{"name":TOOL,
        "description":"Ask the person one to four questions, each with a few choices, when their answer changes what you do next and you cannot reasonably assume it: which of several directions to take, a preference, or a missing detail. They pick from your options or write their own. Your reply ends with the questions, and their answers arrive as their next message. Do not ask what you can find out or decide sensibly yourself, and do not list the same options again in your text.",
        "parameters":{"type":"object","properties":{
            "questions":{"type":"array","minItems":1,"maxItems":MAX_QUESTIONS,"items":{"type":"object","properties":{
                "header":{"type":"string","description":"A label of one to three words, such as First task"},
                "question":{"type":"string","description":"The whole question, ending with a question mark"},
                "options":{"type":"array","minItems":2,"maxItems":MAX_OPTIONS,"items":{"type":"object","properties":{
                    "label":{"type":"string","description":"The choice, in a few words"},
                    "description":{"type":"string","description":"What choosing it means, in one sentence"},
                    "recommended":{"type":"boolean","description":"True for the one option you recommend, if any"}},
                    "required":["label"],"additionalProperties":false}},
                "multiple":{"type":"boolean","description":"True when the person may pick several options"}},
                "required":["question","options"],"additionalProperties":false}}},
        "required":["questions"],"additionalProperties":false}}})
}

fn text(value: &Value, max: usize) -> String {
    value
        .as_str()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Checks a call's arguments and returns the questions as the page shows them, or
/// what the model must fix.
pub fn parse(arguments: &str) -> Result<Vec<Value>, String> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|_| "The arguments were not valid JSON.".to_string())?;
    let items = args["questions"]
        .as_array()
        .filter(|q| !q.is_empty())
        .ok_or("Pass questions: a list of one to four questions.")?;
    if items.len() > MAX_QUESTIONS {
        return Err(format!("Ask at most {MAX_QUESTIONS} questions at once."));
    }
    let mut questions = vec![];
    for (n, item) in items.iter().enumerate() {
        let question = text(&item["question"], 300);
        if question.is_empty() {
            return Err(format!("Question {} has no text.", n + 1));
        }
        let mut options: Vec<Value> = vec![];
        let mut recommended = false;
        for option in item["options"].as_array().into_iter().flatten() {
            let label = text(&option["label"], 80);
            if label.is_empty() || options.iter().any(|o| o["label"] == label.as_str()) {
                continue;
            }
            // Only the first recommendation counts.
            let recommend = option["recommended"].as_bool() == Some(true) && !recommended;
            recommended |= recommend;
            options.push(json!({"label":label,"description":text(&option["description"], 240),"recommended":recommend}));
        }
        if options.len() < 2 {
            return Err(format!(
                "Question {} needs at least two different options.",
                n + 1
            ));
        }
        options.truncate(MAX_OPTIONS);
        let header = text(&item["header"], 30);
        questions.push(json!({
            "header": if header.is_empty() { format!("Question {}", n + 1) } else { header },
            "question": question,
            "options": options,
            "multiple": item["multiple"].as_bool() == Some(true),
        }));
    }
    Ok(questions)
}

/// The activity entry the page shows as the card.
pub fn entry(call_id: &str, questions: Vec<Value>) -> Value {
    let summary = if questions.len() == 1 {
        "Asked a question".to_string()
    } else {
        format!("Asked {} questions", questions.len())
    };
    json!({"id":call_id,"kind":"question","tool":TOOL,"status":"asked","summary":summary,"questions":questions})
}

/// What the model reads back: the turn ends here.
pub const SHOWN: &str = "The person sees your questions now. End your reply without repeating them; their answers arrive as their next message.";

/// Offered when the person may have workspaces but this chat has none: the model
/// asks for one, and the card's button turns it on and carries on.
pub const WORKSPACE_TOOL: &str = "request_workspace";

/// Said in the system prompt whenever ask_user is offered: from the tool list alone,
/// many models write their questions into the reply, where the person cannot pick.
pub const ASK_PROMPT: &str = "When you need the person to choose or tell you something before you can go on well, ask with ask_user instead of writing the questions in your reply: they answer by picking an option. Ask only what changes what you do. To get a workspace, call request_workspace rather than asking.";
/// Said in the system prompt while the tool is offered: from the tool list alone,
/// models tell the person they cannot run code, or act out the program's output.
pub const WORKSPACE_PROMPT: &str = "This chat has no workspace yet, so you cannot run code, open a terminal or keep files. One can be turned on: when the person wants something built, run, tested or previewed, or asks for the workspace or a terminal, call request_workspace. Do not say you are unable to run code, and never give a program's output that did not come from running it: not acted out, and not worked out by hand either, since hashes, random values and long calculations written out that way are wrong.";

pub fn workspace_definition() -> Value {
    json!({"type":"function","function":{"name":WORKSPACE_TOOL,
        "description":"Ask the person to turn on a workspace for this chat: a project folder in their own Linux container where you can write files, run commands and code, and show a live preview. Call it when the request needs that, such as building or running an app, running code or experiments, keeping files, or research, and you cannot do it well without one. Do not call it for questions you can answer in text. Your reply ends with a button that turns the workspace on; once they press it, you continue in their next message with the workspace tools.",
        "parameters":{"type":"object","properties":{
            "reason":{"type":"string","description":"Why this needs a workspace, in one short sentence the person reads, such as \"To build the app and show you a live preview.\""}},
        "required":["reason"],"additionalProperties":false}}})
}

/// The card for a workspace request, or what the model must fix.
pub fn workspace_entry(call_id: &str, arguments: &str) -> Value {
    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
    let reason = text(&args["reason"], 240);
    json!({"id":call_id,"kind":"workspace","tool":WORKSPACE_TOOL,"status":"asked","summary":"Asked for a workspace","reason":reason})
}

/// Offered when the person may run research but this chat isn't doing any: the
/// model can suggest it, and the card's button sets the chat up for research.
pub const RESEARCH_TOOL: &str = "suggest_research";

/// Said in the system prompt while the tool is offered: from the tool list alone, models
/// start a long tuning job in the chat itself, where nothing keeps or compares the tries.
pub const RESEARCH_PROMPT: &str = "When the person wants something pushed as far as it will go over many measured tries, such as tuning or optimizing for speed, accuracy, size or cost, a parameter sweep, or a run left going overnight, call suggest_research before doing that work here. A research project keeps each try on its own branch, runs them the same way and charts the results; work done in a plain chat keeps none of that.";

pub fn research_definition() -> Value {
    json!({"type":"function","function":{"name":RESEARCH_TOOL,
        "description":"Suggest turning this into a research project, where you try several alternatives as git branches, run each with the same command, and keep the best result as the starting point of the next round. Suggest it only when the person wants something measurably better (faster, more accurate, smaller, cheaper) and getting there takes several tries that can be measured by one number, not for a question with a known answer or a one-off change. The person sees a card with a button that moves the chat into a project, turns on a workspace and starts setting up the research. Your reply ends at the card.",
        "parameters":{"type":"object","properties":{
            "goal":{"type":"string","description":"What to improve, in one short sentence, such as \"Make decode at least 5% faster on this node\""},
            "metric":{"type":"string","description":"The number that decides it, such as \"decode tokens per second, higher is better\""}},
        "required":["goal"],"additionalProperties":false}}})
}

/// The card for a research suggestion. `in_project` says the chat's own project can
/// hold the research; otherwise the button makes a new project for it.
pub fn research_entry(call_id: &str, arguments: &str, in_project: bool) -> Value {
    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
    json!({"id":call_id,"kind":"research","tool":RESEARCH_TOOL,"status":"asked","summary":"Suggested research","goal":text(&args["goal"], 200),"metric":text(&args["metric"], 120),"inProject":in_project})
}

pub const WORKSPACE_SHOWN: &str = "The person sees a button to turn on a workspace for this chat. End your reply now in at most one short sentence, without repeating the reason; when they turn it on, their next message continues the task and you will have the workspace tools.";

pub const RESEARCH_SHOWN: &str = "The person sees a card suggesting research, with a button to start it. End your reply now in at most one short sentence, without repeating the goal; if they start it, their next message asks you to set the research up and you will have the workspace and research tools.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_request_carries_a_short_reason() {
        let entry = workspace_entry(
            "call_1",
            &json!({"reason":"To build the app\u{7} and show a preview."}).to_string(),
        );
        assert_eq!(entry["kind"], "workspace");
        assert_eq!(entry["reason"], "To build the app and show a preview.");
        assert_eq!(workspace_entry("call_2", "not json")["reason"], "");
    }

    #[test]
    fn a_research_suggestion_carries_its_goal_and_metric() {
        let entry = research_entry(
            "call_1",
            &json!({"goal":"Make decode faster","metric":"tokens per second"}).to_string(),
            false,
        );
        assert_eq!(entry["kind"], "research");
        assert_eq!(entry["goal"], "Make decode faster");
        assert_eq!(entry["metric"], "tokens per second");
        assert_eq!(entry["inProject"], false);
        assert_eq!(research_entry("call_2", "{}", true)["metric"], "");
    }

    #[test]
    fn keeps_good_questions_and_explains_bad_ones() {
        let questions = parse(
            &json!({"questions":[{"header":"First task","question":"Which first?","options":[
            {"label":"Find it","description":"Search everything","recommended":true},
            {"label":"Critique","recommended":true},
            {"label":"Find it"}]}]})
            .to_string(),
        )
        .unwrap();
        let options = questions[0]["options"].as_array().unwrap();
        assert_eq!(options.len(), 2, "duplicates are dropped");
        assert_eq!(options[0]["recommended"], true);
        assert_eq!(options[1]["recommended"], false, "one recommendation");
        assert_eq!(questions[0]["multiple"], false);
        assert!(
            parse(r#"{"questions":[{"question":"Which?","options":[{"label":"A"}]}]}"#)
                .unwrap_err()
                .contains("two different options")
        );
        assert!(parse(r#"{"questions":[]}"#).is_err());
        assert!(parse("not json").is_err());
        let untitled = parse(
            r#"{"questions":[{"question":"Which?","options":[{"label":"A"},{"label":"B"}]}]}"#,
        )
        .unwrap();
        assert_eq!(untitled[0]["header"], "Question 1");
    }
}
