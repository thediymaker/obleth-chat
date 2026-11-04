//! Questions the model asks the person with a few choices each, shown as a card at
//! the end of the reply. The reply ends at the card; the person's answers arrive as
//! their next message, so an answer still works hours later or after a restart.

use serde_json::{json, Value};

pub const TOOL: &str = "ask_user";
const MAX_QUESTIONS: usize = 4;
const MAX_OPTIONS: usize = 6;

pub fn definition() -> Value {
    json!({"type":"function","function":{"name":TOOL,
        "description":"Ask the person one to four questions, each with a few choices, when their answer changes what you do next and you cannot reasonably assume it: which of several directions to take, a preference, or a missing detail. They pick from your options or write their own. Use it also for a question at the end of a reply, after what you could already give. Your reply ends with the questions, and their answers arrive as their next message. Do not ask what you can find out or decide sensibly yourself, and do not list the same options again in your text.",
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
pub const ASK_PROMPT: &str = "When you need the person to choose or tell you something before you can go on well, ask with ask_user instead of writing the questions in your reply: they answer by picking an option. That includes a reply that gives what it can and ends by asking which of several directions to take or for a missing detail: write the rest, then put that question and its choices in ask_user, not in the text. Ask only what changes what you do. To get a workspace, call request_workspace rather than asking.";
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

/// Offered to administrators: the model can suggest turning what the person describes
/// into an app, and the card's button makes the project and app with its steps.
pub const APP_TOOL: &str = "suggest_app";

/// Said in the system prompt while the tool is offered: from the tool list alone, models
/// say they cannot host anything and send the person off to build a server.
pub const APP_PROMPT: &str = "Chat can turn a project into an app: a set of steps over typed inputs (models, the project's knowledge collections, JavaScript, rules, HTTP requests) that answers one request at a time, over an HTTP API with a key and on a chat page. When the person wants something that takes a question or input and gives back an answer, for others or for other programs, such as an assistant over their documents, an API for their lab, or a tool for their group, call suggest_app with a first version of its steps instead of saying you cannot host or expose it, or planning a server for them to run. An app is not a website: it cannot list, show or hand out files, serve pages or keep what people add. For a portal, a site for sharing files or data, or a dashboard, build it in a workspace instead, and deploy it on the person's cluster only when they ask.";

pub fn app_definition() -> Value {
    json!({"type":"function","function":{"name":APP_TOOL,
        "description":"Suggest making an app. The person sees a card with the app's name and steps and a button that creates it, in this chat's project or in a new one, together with a knowledge collection for its documents if it needs one; they then add documents, try it and publish it from the project's App tab. Your reply ends at the card.\n\nThe definition is {inputs, steps}. inputs: [{name, type: text|number|boolean|json, required, description}]; names and step ids use lowercase letters, digits and _. Each step has type, id and title, plus:\n- agent: model (leave empty for this chat's model), instructions, prompt, json (true to read the reply as JSON)\n- search: query, limit (searches the project's knowledge)\n- code: code (a JavaScript function body that returns a value; it reads inputs, steps, item and index, with no network or files)\n- decide: model, question, input, paths: [{name, description, steps}], otherwise: [steps]; the model picks one path\n- if: paths: [{when: [{value, test: is_empty|not_empty|equals|not_equals|contains|more_than|less_than, to}], steps}], otherwise: [steps]\n- for_each: items, steps, concurrency\n- http: method, url, headers, body (public addresses only)\n- response: value (what the app answers; ends the run)\nText reads earlier results with references such as {{inputs.question}}, {{steps.search}} and {{item}}; a string that is only a reference takes the value itself. A chatbot over documents is usually search, then agent with the passages in its prompt, then response.",
        "parameters":{"type":"object","properties":{
            "name":{"type":"string","description":"The app's name, such as \"Assay Lab assistant\""},
            "description":{"type":"string","description":"What it does, in one sentence"},
            "documents":{"type":"boolean","description":"True when it answers from documents the person will upload"},
            "definition":{"type":"object","description":"The inputs and steps, as described above"}},
        "required":["name","description","definition"],"additionalProperties":false}}})
}

/// Fills an empty or missing model on agent and decide steps with the chat's own.
fn fill_models(steps: Option<&mut Value>, model: &str) {
    for step in steps.and_then(Value::as_array_mut).into_iter().flatten() {
        let Some(step) = step.as_object_mut() else {
            continue;
        };
        if matches!(
            step.get("type").and_then(Value::as_str),
            Some("agent" | "decide")
        ) && step
            .get("model")
            .and_then(Value::as_str)
            .is_none_or(|m| m.trim().is_empty())
        {
            step.insert("model".into(), json!(model));
        }
        fill_models(step.get_mut("steps"), model);
        fill_models(step.get_mut("otherwise"), model);
        for path in step
            .get_mut("paths")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            fill_models(path.get_mut("steps"), model);
        }
    }
}

/// The card for an app suggestion, or what the model must fix. `in_project` says the
/// chat's own project takes the app; otherwise the button makes a new project.
pub fn app_entry(
    call_id: &str,
    arguments: &str,
    model: &str,
    in_project: bool,
) -> Result<Value, String> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|_| "The arguments were not valid JSON.".to_string())?;
    let name = text(&args["name"], 120);
    if name.is_empty() {
        return Err("Give the app a name.".into());
    }
    let mut definition = args["definition"].clone();
    if !definition.is_object() {
        return Err("Pass definition: an object with inputs and steps.".into());
    }
    fill_models(definition.get_mut("steps"), model);
    let parsed: crate::apps::engine::Definition = serde_json::from_value(definition)
        .map_err(|e| format!("The definition does not fit the format: {e}"))?;
    parsed.validate()?;
    if parsed.steps.is_empty() {
        return Err("Give the app at least one step.".into());
    }
    Ok(
        json!({"id":call_id,"kind":"app","tool":APP_TOOL,"status":"asked","summary":"Suggested an app","name":name,"description":text(&args["description"], 300),"documents":args["documents"].as_bool() == Some(true),"definition":parsed,"inProject":in_project}),
    )
}

pub const APP_SHOWN: &str = "The person sees a card with the app and a button to create it. End your reply now in at most one short sentence, without repeating the steps; once it is created they add documents, try it and publish it from the project's App tab.";

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
    fn an_app_suggestion_gets_the_chat_model_and_explains_bad_steps() {
        let arguments = json!({"name":"Assay Lab","description":"Answers from the lab's protocols","documents":true,"definition":{
            "inputs":[{"name":"question","type":"text","required":true}],
            "steps":[{"type":"search","id":"search","query":"{{inputs.question}}"},
                {"type":"decide","id":"route","input":"{{inputs.question}}","paths":[{"name":"Samples","steps":[{"type":"agent","id":"agent","model":"","prompt":"{{steps.search}}"}]}]},
                {"type":"response","id":"answer","value":"{{steps.route}}"}]}})
        .to_string();
        let entry = app_entry("call_1", &arguments, "glm-5-3", true).unwrap();
        assert_eq!(entry["kind"], "app");
        assert_eq!(entry["documents"], true);
        assert_eq!(entry["inProject"], true);
        assert_eq!(entry["definition"]["steps"][1]["model"], "glm-5-3");
        assert_eq!(
            entry["definition"]["steps"][1]["paths"][0]["steps"][0]["model"],
            "glm-5-3"
        );
        let unknown =
            json!({"name":"A","description":"","definition":{"steps":[{"type":"email","id":"x"}]}})
                .to_string();
        assert!(app_entry("c", &unknown, "m", false)
            .unwrap_err()
            .contains("does not fit"));
        let bad_id = json!({"name":"A","description":"","definition":{"steps":[{"type":"code","id":"Shape","code":"return 1;"}]}}).to_string();
        assert!(app_entry("c", &bad_id, "m", false)
            .unwrap_err()
            .contains("Shape"));
        let empty = json!({"name":"A","description":"","definition":{"steps":[]}}).to_string();
        assert!(app_entry("c", &empty, "m", false).is_err());
        assert!(app_entry("c", "{}", "m", false).is_err());
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
