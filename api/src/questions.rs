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

#[cfg(test)]
mod tests {
    use super::*;

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
