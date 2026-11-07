//! Pausing at the person's request. When they ask the model to stop, pause or wait,
//! often in a note sent while it works, the model calls pause_work with where the
//! work stands. Chat writes that to PAUSED.md in the workspace, checkpoints it, puts
//! it at the end of the reply and ends the turn there: no check of the work, no
//! report check, and Auto mode leaves the chat alone until the person writes again.

use serde_json::{json, Value};

pub const TOOL: &str = "pause_work";
/// The note's file in the project folder, read back at the start of later turns.
pub const FILE: &str = "PAUSED.md";
const MAX_FIELD: usize = 6000;
/// Shorter than this, `next` cannot say how to pick the work up.
const MIN_NEXT: usize = 20;

pub fn definition() -> Value {
    json!({"type":"function","function":{"name":TOOL,
        "description":"Pause the work because the person asked you to stop, pause, hold or wait, including in a note they send while you work. Before calling it, bring the work to a clean stopping point: let the current step finish or stop the servers, scripts and background jobs you started, so nothing is left half-written. Then call this with where things stand, written for someone who picks the work up later without this conversation. Chat saves it to PAUSED.md in the workspace with a checkpoint, shows it to the person and ends your reply: nothing runs after this call, and the work stays paused until the person writes again. Do not call it unless the person asked you to stop.",
        "parameters":{"type":"object","properties":{
            "done":{"type":"string","description":"What is finished, with the files, results and numbers that matter."},
            "state":{"type":"string","description":"Where it stopped: what was in progress and how far it got, what you stopped, what is still running (research runs, servers), and anything half-done to check."},
            "next":{"type":"string","description":"The next steps in order, with the exact commands, files, branches and ids needed to carry on."}},
        "required":["state","next"],"additionalProperties":false}}})
}

/// Where the work stands, as the model gave it.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub done: String,
    pub state: String,
    pub next: String,
}

fn field(args: &Value, key: &str) -> String {
    let text: String = args[key]
        .as_str()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(MAX_FIELD)
        .collect();
    text.trim().to_owned()
}

/// Checks a call's arguments; an error says what the model must fill in.
pub fn parse(arguments: &str) -> Result<Note, String> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|_| "The arguments were not valid JSON. Nothing was paused.".to_string())?;
    let note = Note {
        done: field(&args, "done"),
        state: field(&args, "state"),
        next: field(&args, "next"),
    };
    if note.state.is_empty() {
        return Err("Nothing was paused: say in state where the work stopped, what you stopped and what is still running.".into());
    }
    if note.next.chars().count() < MIN_NEXT {
        return Err("Nothing was paused: list in next the steps to carry on, with the commands, files and ids they need.".into());
    }
    Ok(note)
}

impl Note {
    /// PAUSED.md, for whoever carries on: possibly a later chat without this one's history.
    pub fn file(&self, at: chrono::DateTime<chrono::Utc>) -> String {
        let mut text = format!(
            "# Paused\n\nThe person paused this work on {} UTC. Carry on from here when they ask, then delete this file once the work is going again.\n\n",
            at.format("%Y-%m-%d %H:%M")
        );
        if !self.done.is_empty() {
            text.push_str(&format!("## Done\n\n{}\n\n", self.done));
        }
        text.push_str(&format!(
            "## Where it stopped\n\n{}\n\n## Next\n\n{}\n",
            self.state, self.next
        ));
        text
    }

    /// The end of the reply: the person sees where the work stands, and so does the
    /// model when the chat carries on.
    pub fn reply(&self, saved: bool) -> String {
        let mut text = String::from("**Paused.** ");
        text.push_str(if saved {
            "Where the work stands is saved in `PAUSED.md` in the workspace, with a checkpoint."
        } else {
            "Where the work stands is below."
        });
        text.push_str(" Write when you want to carry on.\n\n");
        if !self.done.is_empty() {
            text.push_str(&format!("**Done**\n\n{}\n\n", self.done));
        }
        text.push_str(&format!(
            "**Where it stopped**\n\n{}\n\n**Next**\n\n{}",
            self.state, self.next
        ));
        text
    }
}

/// What the model is told once the pause is saved. The turn ends after this round.
pub const SHOWN: &str = "Paused. Chat saved your note and ends your reply here; the person writes when they want the work to carry on.";

/// The step under the reply.
pub fn entry(call_id: &str, saved: Option<&str>) -> Value {
    let detail = match saved {
        Some(_) => format!("Saved where the work stands to {FILE}"),
        None => "Where the work stands is in the reply".into(),
    };
    json!({"id":call_id,"tool":TOOL,"summary":"Paused at your request","detail":detail,"status":"completed","startedAt":chrono::Utc::now()})
}

/// The start-of-turn line for a workspace whose work was paused.
pub fn context(text: &str) -> String {
    format!("\nThis work was paused, and {FILE} in the project folder says where it stopped. When the person asks to carry on, start from its next steps rather than redoing work, and delete {FILE} once the work is going again:\n{text}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_where_it_stopped_and_what_comes_next() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{"next":"Run build_dataset.py --resume, then label."}"#).is_err());
        assert!(parse(r#"{"state":"Stopped the builder.","next":"go on"}"#).is_err());
        let note = parse(r#"{"done":" 31 records built. ","state":"Builder stopped at commit 565f36d.\u0007","next":"Run python build_dataset.py --resume from data/."}"#).unwrap();
        assert_eq!(note.done, "31 records built.");
        assert_eq!(note.state, "Builder stopped at commit 565f36d.");
    }

    #[test]
    fn writes_the_note_for_a_restart() {
        let note = Note {
            done: String::new(),
            state: "Stopped the builder.".into(),
            next: "Run build_dataset.py --resume.".into(),
        };
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-07T18:32:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let file = note.file(at);
        assert!(
            file.starts_with("# Paused\n\nThe person paused this work on 2026-11-07 18:32 UTC.")
        );
        assert!(!file.contains("## Done"), "no empty section");
        assert!(file.ends_with("## Next\n\nRun build_dataset.py --resume.\n"));
        let reply = note.reply(true);
        assert!(reply.contains("`PAUSED.md`"));
        assert!(reply.ends_with("**Next**\n\nRun build_dataset.py --resume."));
        // Auto mode resumes a reply that ends with its own paused line; this one must not.
        assert_eq!(
            crate::autopilot::nudge_for("completed", None, &reply, false),
            None
        );
        assert!(!note.reply(false).contains("PAUSED.md"));
    }
}
