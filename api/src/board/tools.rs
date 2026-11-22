//! The board in a chat that is in a project: the model reads it with `board_list`,
//! adds cards with `board_add` and works them with `board_update`. They change only
//! the project's own board, so like save_item they run without asking. The system
//! prompt carries a short view of the board, with the card the chat works in full,
//! so ideas the person adds and answers they give reach the model on its next turn.

use super::{Actor, Card, Change, NewCard, Step};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

pub const LIST: &str = "board_list";
pub const ADD: &str = "board_add";
pub const UPDATE: &str = "board_update";
pub const NAMES: [&str; 3] = [LIST, ADD, UPDATE];
pub fn is_tool(name: &str) -> bool {
    NAMES.contains(&name)
}
/// Cards per column the prompt names; board_list names more.
const PROMPT_CARDS: usize = 8;
const LIST_CARDS: usize = 40;

/// A column as the person sees it named.
pub fn column(status: &str) -> &'static str {
    match status {
        "suggested" => "Suggested",
        "idea" => "Ideas",
        "next" => "Up next",
        "doing" => "Now",
        "waiting" => "Needs you",
        "done" => "Done",
        _ => "Dismissed",
    }
}

/// The model's word for a column, to the status it means.
fn status_of(word: &str) -> Option<&'static str> {
    match word
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-'], "_")
        .as_str()
    {
        "ideas" | "idea" => Some("idea"),
        "next" | "up_next" => Some("next"),
        "now" | "doing" => Some("doing"),
        "needs_you" | "waiting" => Some("waiting"),
        "done" => Some("done"),
        "suggested" => Some("suggested"),
        _ => None,
    }
}

pub fn definitions() -> Vec<Value> {
    vec![
        json!({"type":"function","function":{"name":LIST,
            "description":"Read the project's board: every card by column (Ideas, Up next in order, Now, Needs you, the latest Done) with its id, who is on it, its steps and progress. Give a card id to read that card in full with its notes.",
            "parameters":{"type":"object","properties":{
                "card":{"type":"string","description":"A card's id (the 8 characters in brackets) to read in full"}},
                "additionalProperties":false}}}),
        json!({"type":"function","function":{"name":ADD,
            "description":"Add a card to the project's board: something to do, such as an experiment to run, a section to write or a level to design. Add cards when the person asks you to note, plan or track work, or when splitting a big task into cards helps. Ideas holds what is not decided; Up next is what should be done next, in order.",
            "parameters":{"type":"object","properties":{
                "title":{"type":"string","description":"What to do, in a short sentence, such as: Add a limitations paragraph"},
                "kind":{"type":"string","description":"A one- or two-word label in the project's terms, such as Experiment, Writing, Figures, Level or Quest"},
                "notes":{"type":"string","description":"What the card is for and what done looks like, in a few sentences"},
                "column":{"type":"string","enum":["ideas","next","suggested"],"description":"ideas (the default), next when the person wants it done soon, or suggested for an idea of your own they did not ask for: they keep or dismiss it"},
                "steps":{"type":"array","items":{"type":"string"},"description":"Up to 12 steps, when they are clear already"}},
                "required":["title"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":UPDATE,
            "description":"Change a card on the project's board as you work it: set its steps and a one-line progress note, move it to now when you start it in this chat, to done when it is finished, or back to ideas or next. When you need the person to decide something before you can go on, give a question and two to four options: the card moves to Needs you, and you stop and wait for their answer.",
            "parameters":{"type":"object","properties":{
                "card":{"type":"string","description":"The card's id, the 8 characters in brackets"},
                "steps":{"type":"array","items":{"type":"object","properties":{
                    "text":{"type":"string"},
                    "state":{"type":"string","enum":["todo","now","done"]}},
                    "required":["text","state"],"additionalProperties":false},
                    "description":"The card's steps, all of them, up to 12, each todo, now or done"},
                "progress":{"type":"string","description":"One line on where the work stands, such as: 3 of 4 runs back"},
                "notes":{"type":"string","description":"New notes for the card, replacing the old ones"},
                "move_to":{"type":"string","enum":["ideas","next","now","done"],"description":"now puts you on the card in this chat; done when it is finished"},
                "question":{"type":"string","description":"What the person must decide, as one question"},
                "options":{"type":"array","items":{"type":"string"},"description":"Two to four short answers to pick from"}},
                "required":["card"],"additionalProperties":false}}}),
    ]
}

/// What a tool step says while it runs.
pub fn summary(name: &str, args: &Value) -> String {
    match name {
        LIST => "Reading the board".into(),
        ADD => format!(
            "Adding “{}” to the board",
            excerpt(args["title"].as_str().unwrap_or("a card"), 60)
        ),
        _ => "Updating a card".into(),
    }
}

pub struct Ctx<'a> {
    pub pool: &'a PgPool,
    pub user: Uuid,
    pub conversation: Uuid,
    pub project: Uuid,
}

/// What a board tool did: whether it worked, the trace's line, what the model reads,
/// and the card it touched.
pub struct Ran {
    pub ok: bool,
    pub summary: String,
    pub output: String,
    pub card: Option<Uuid>,
}
impl Ran {
    fn failed(summary: &str, output: impl Into<String>) -> Self {
        Self {
            ok: false,
            summary: summary.into(),
            output: output.into(),
            card: None,
        }
    }
}

pub async fn run(ctx: &Ctx<'_>, name: &str, arguments: &str) -> Ran {
    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
    match name {
        LIST => list(ctx, &args).await,
        ADD => add(ctx, &args).await,
        UPDATE => update(ctx, &args).await,
        _ => Ran::failed("Unknown tool", format!("There is no tool named {name}.")),
    }
}

fn short(id: Uuid) -> String {
    id.simple().to_string()[..8].to_owned()
}

fn excerpt(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The card an id names, from its first characters.
async fn find(ctx: &Ctx<'_>, id: &str) -> Result<Card, String> {
    let id: String = id
        .trim()
        .trim_matches(['[', ']'])
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_lowercase();
    if id.len() < 4 {
        return Err("Give the card's id: the 8 characters in brackets on the board.".into());
    }
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM project_cards WHERE project_id = $1 AND replace(id::text, '-', '') LIKE $2 || '%' LIMIT 2",
    )
    .bind(ctx.project)
    .bind(&id)
    .fetch_all(ctx.pool)
    .await
    .map_err(|_| "The board could not be read.".to_string())?;
    match ids.as_slice() {
        [one] => super::card(ctx.pool, ctx.user, ctx.project, *one)
            .await
            .map_err(|e| e.1),
        [] => Err(format!(
            "No card on this board has the id {id}. Call {LIST} to see the cards."
        )),
        _ => Err("More than one card starts with that; give all 8 characters.".into()),
    }
}

/// Who is on a card, as the model reads it.
fn who(card: &Card, conversation: Uuid) -> Option<String> {
    match card.worker.as_deref() {
        Some("assistant") if card.chat_id == Some(conversation) => Some("you, in this chat".into()),
        Some("assistant") => Some("the assistant, in another chat".into()),
        Some(_) => Some(
            card.worker_name
                .clone()
                .unwrap_or_else(|| "a person".into()),
        ),
        None => None,
    }
}

fn steps_line(steps: &[Step]) -> Option<String> {
    if steps.is_empty() {
        return None;
    }
    let done = steps.iter().filter(|s| s.state == "done").count();
    Some(format!("{done} of {} steps done", steps.len()))
}

/// A card on one line: id, title, kind and how it stands.
fn line(card: &Card, conversation: Uuid) -> String {
    let mut text = format!("[{}] “{}”", short(card.id), excerpt(&card.title, 100));
    if !card.kind.is_empty() {
        text.push_str(&format!(" ({})", card.kind));
    }
    let mut facts = vec![];
    if card.made_by == "assistant" && matches!(card.status.as_str(), "suggested" | "idea") {
        facts.push("made by the assistant".to_owned());
    }
    if card.status != "done" {
        if let Some(who) = who(card, conversation) {
            facts.push(format!("on it: {who}"));
        }
        if let Some(steps) = steps_line(&card.steps()) {
            facts.push(steps);
        }
        if !card.progress.is_empty() {
            facts.push(format!("progress: {}", excerpt(&card.progress, 120)));
        }
    }
    if card.status == "waiting" && !card.question.is_empty() {
        facts.push(format!("asks: {}", excerpt(&card.question, 160)));
    }
    if !facts.is_empty() {
        text.push_str(" — ");
        text.push_str(&facts.join("; "));
    }
    text
}

/// A card in full, as the model reads it.
fn full(card: &Card, conversation: Uuid) -> String {
    let mut text = format!(
        "[{}] “{}”{} — in {}",
        short(card.id),
        card.title,
        if card.kind.is_empty() {
            String::new()
        } else {
            format!(" ({})", card.kind)
        },
        column(&card.status)
    );
    if let Some(who) = who(card, conversation) {
        text.push_str(&format!(", on it: {who}"));
    }
    text.push('\n');
    if !card.notes.is_empty() {
        text.push_str(&format!("Notes: {}\n", excerpt(&card.notes, 1500)));
    }
    let steps = card.steps();
    if !steps.is_empty() {
        text.push_str("Steps:\n");
        for (i, step) in steps.iter().enumerate() {
            text.push_str(&format!("{}. [{}] {}\n", i + 1, step.state, step.text));
        }
    }
    if !card.progress.is_empty() {
        text.push_str(&format!("Progress: {}\n", card.progress));
    }
    if !card.question.is_empty() {
        text.push_str(&format!("Question asked: {}\n", card.question));
        let options = card.options();
        if !options.is_empty() {
            text.push_str(&format!("Options: {}\n", options.join(" / ")));
        }
        if card.answer.is_empty() {
            text.push_str("Not answered yet.\n");
        } else {
            text.push_str(&format!(
                "The person answered: {}. Go on with that answer.\n",
                card.answer.trim_end_matches('.')
            ));
        }
    }
    text.trim_end().to_owned()
}

/// The board by column, `per` cards a column.
fn columns(cards: &[Card], done_count: i64, conversation: Uuid, per: usize) -> String {
    let mut text = String::new();
    for status in ["doing", "waiting", "next", "idea", "suggested", "done"] {
        let here: Vec<&Card> = cards.iter().filter(|c| c.status == status).collect();
        let total = if status == "done" {
            done_count as usize
        } else {
            here.len()
        };
        if total == 0 {
            continue;
        }
        let label = if status == "done" {
            format!("Done ({total}, latest first)")
        } else {
            format!("{} ({total})", column(status))
        };
        text.push_str(&format!("{label}:\n"));
        for (i, card) in here.iter().take(per).enumerate() {
            let number = if status == "next" {
                format!("{}. ", i + 1)
            } else {
                "- ".into()
            };
            text.push_str(&format!("{number}{}\n", line(card, conversation)));
        }
        let shown = here.len().min(per);
        if total > shown {
            text.push_str(&format!("…and {} more\n", total - shown));
        }
    }
    text.trim_end().to_owned()
}

async fn list(ctx: &Ctx<'_>, args: &Value) -> Ran {
    if let Some(id) = args["card"].as_str().filter(|s| !s.trim().is_empty()) {
        return match find(ctx, id).await {
            Ok(card) => Ran {
                ok: true,
                summary: format!("Read “{}”", excerpt(&card.title, 60)),
                output: full(&card, ctx.conversation),
                card: Some(card.id),
            },
            Err(problem) => Ran::failed("No such card", problem),
        };
    }
    match super::board(ctx.pool, ctx.user, ctx.project).await {
        Ok((cards, _)) if cards.is_empty() => Ran {
            ok: true,
            summary: "Read the board: no cards yet".into(),
            output: "The board has no cards yet.".into(),
            card: None,
        },
        Ok((cards, done_count)) => {
            let open = cards.iter().filter(|c| c.status != "done").count();
            Ran {
                ok: true,
                summary: format!(
                    "Read the board: {open} open card{}",
                    if open == 1 { "" } else { "s" }
                ),
                output: columns(&cards, done_count, ctx.conversation, LIST_CARDS),
                card: None,
            }
        }
        Err(error) => Ran::failed(
            "Could not read the board",
            format!("The board could not be read: {}", error.1),
        ),
    }
}

/// Steps the model gave, as strings or as {text, state}.
fn steps_of(value: &Value) -> Option<Vec<Step>> {
    let list = value.as_array()?;
    Some(
        list.iter()
            .filter_map(|s| match s {
                Value::String(text) => Some(Step {
                    text: text.clone(),
                    state: "todo".into(),
                }),
                Value::Object(o) => Some(Step {
                    text: o.get("text")?.as_str()?.to_owned(),
                    state: o
                        .get("state")
                        .and_then(Value::as_str)
                        .unwrap_or("todo")
                        .to_owned(),
                }),
                _ => None,
            })
            .collect(),
    )
}

async fn add(ctx: &Ctx<'_>, args: &Value) -> Ran {
    let title = args["title"].as_str().unwrap_or("").trim();
    let status = match args["column"].as_str() {
        None => "idea",
        Some(word) => match status_of(word).filter(|s| matches!(*s, "idea" | "next" | "suggested"))
        {
            Some(status) => status,
            None => {
                return Ran::failed(
                    "Could not add the card",
                    "A new card goes into ideas, next or suggested.",
                )
            }
        },
    };
    let new = NewCard {
        title: title.to_owned(),
        notes: args["notes"].as_str().unwrap_or("").to_owned(),
        kind: args["kind"].as_str().unwrap_or("").to_owned(),
        status: Some(status.to_owned()),
        steps: steps_of(&args["steps"]).unwrap_or_default(),
        source_chat: Some(ctx.conversation),
    };
    match super::make(ctx.pool, ctx.project, Actor::assistant(ctx.user), new).await {
        Ok(id) => Ran {
            ok: true,
            summary: format!("Added “{}” to {}", excerpt(title, 60), column(status)),
            output: format!(
                "Added “{title}” to {} as [{}]. The person sees it on the project's board.",
                column(status),
                short(id)
            ),
            card: Some(id),
        },
        Err(error) => Ran::failed(
            "Could not add the card",
            format!("Could not add the card: {}", error.1),
        ),
    }
}

async fn update(ctx: &Ctx<'_>, args: &Value) -> Ran {
    let card = match find(ctx, args["card"].as_str().unwrap_or("")).await {
        Ok(card) => card,
        Err(problem) => return Ran::failed("No such card", problem),
    };
    let title = excerpt(&card.title, 60);
    let question = args["question"]
        .as_str()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let move_to = match args["move_to"].as_str() {
        None => None,
        Some(word) => {
            match status_of(word).filter(|s| matches!(*s, "idea" | "next" | "doing" | "done")) {
                Some(status) => Some(status),
                None => {
                    return Ran::failed(
                        "Could not update the card",
                        "move_to is ideas, next, now or done.",
                    )
                }
            }
        }
    };
    if question.is_some() && move_to.is_some() {
        return Ran::failed(
            "Could not update the card",
            "Either ask a question (the card moves to Needs you) or move the card, not both.",
        );
    }
    let mut change = Change {
        steps: steps_of(&args["steps"]),
        progress: args["progress"].as_str().map(str::to_owned),
        notes: args["notes"].as_str().map(str::to_owned),
        ..Default::default()
    };
    if let Some(question) = question {
        let options: Vec<String> = args["options"]
            .as_array()
            .map(|o| {
                o.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        change.question = Some(question.to_owned());
        change.options = Some(options);
    }
    match move_to {
        Some("doing") => {
            // Someone else's chat writing on the card now keeps it.
            if card.status == "doing"
                && card.chat_working
                && card.chat_id.is_some_and(|c| c != ctx.conversation)
            {
                return Ran::failed(
                    "The card is being worked on elsewhere",
                    "Another chat is working this card right now. Leave it, or ask the person.",
                );
            }
            change.chat_id = Some(ctx.conversation);
        }
        Some(status) => change.status = Some(status.to_owned()),
        None => {}
    }
    if change.steps.is_none()
        && change.progress.is_none()
        && change.notes.is_none()
        && change.question.is_none()
        && move_to.is_none()
    {
        return Ran::failed(
            "Could not update the card",
            "Say what to change: steps, progress, notes, move_to, or a question.",
        );
    }
    let mut did = vec![];
    if let Some(steps) = &change.steps {
        let done = steps.iter().filter(|s| s.state == "done").count();
        did.push(format!("steps set ({done} of {} done)", steps.len()));
    }
    if change.progress.is_some() {
        did.push("progress noted".to_owned());
    }
    if change.notes.is_some() {
        did.push("notes replaced".to_owned());
    }
    let asked = change.question.is_some();
    match super::change(
        ctx.pool,
        ctx.project,
        card.id,
        Actor::assistant(ctx.user),
        change,
    )
    .await
    {
        Ok(()) => {
            let (summary, output) = if asked {
                (
                    format!("Asked about “{title}”"),
                    format!("Moved “{title}” to Needs you with your question. Tell the person what you asked in a sentence and stop here: their answer comes back to you with the board on your next turn."),
                )
            } else if let Some(status) = move_to {
                did.push(format!("moved to {}", column(status)));
                let summary = match status {
                    "done" => format!("Marked “{title}” done"),
                    "doing" => format!("Started “{title}”"),
                    _ => format!("Moved “{title}” to {}", column(status)),
                };
                (summary, format!("“{title}”: {}.", did.join(", ")))
            } else {
                (
                    format!("Updated “{title}”"),
                    format!("“{title}”: {}.", did.join(", ")),
                )
            };
            Ran {
                ok: true,
                summary,
                output,
                card: Some(card.id),
            }
        }
        Err(error) => Ran::failed(
            "Could not update the card",
            format!("Could not update “{title}”: {}", error.1),
        ),
    }
}

/// The board's part of the system prompt: how to use the tools when the model has
/// them, the card this chat works in full, and the rest by column. None for a chat
/// without tools on an empty board.
pub async fn prompt(
    pool: &PgPool,
    user: Uuid,
    project: Uuid,
    conversation: Uuid,
    tools: bool,
) -> Option<String> {
    let (cards, done_count) = match super::board(pool, user, project).await {
        Ok(board) => board,
        Err(error) => {
            tracing::warn!("could not read a project's board: {}", error.1);
            return None;
        }
    };
    if cards.is_empty() && !tools {
        return None;
    }
    let mut text = String::from("# The project's board\nThe project keeps a board of cards: work that the people in it and you make and get through. Ideas are not decided yet; Up next is in order; Now is being worked on; Needs you waits on the person; Done is finished.");
    if tools {
        text.push_str(&format!(" Read it with {LIST}. When the person asks you to note, plan or track work, add cards with {ADD}. While you work a card, keep it current with {UPDATE}: its steps, a one-line progress note, and done when it is finished. When you need the person to decide something before you can go on, give {UPDATE} a question and options, then stop. Refer to cards by title when you talk to the person; ids in brackets are for the tools."));
    } else {
        text.push_str(" You cannot change the board in this chat; refer to cards by title.");
    }
    let here = cards
        .iter()
        .filter(|c| c.chat_id == Some(conversation))
        .min_by_key(|c| match c.status.as_str() {
            "doing" | "waiting" => 0,
            "done" => 2,
            _ => 1,
        });
    let here_id = here.map(|c| c.id);
    if let Some(card) = here {
        text.push_str("\n\n## The card this chat is working on\n");
        text.push_str(&full(card, conversation));
    }
    if cards.is_empty() {
        text.push_str("\n\nThe board has no cards yet.");
        return Some(text);
    }
    let others: Vec<Card> = cards
        .into_iter()
        .filter(|c| Some(c.id) != here_id)
        .collect();
    if !others.is_empty() {
        text.push_str("\n\n## On the board\n");
        text.push_str(&columns(&others, done_count, conversation, PROMPT_CARDS));
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Database;

    struct Fixture {
        db: Database,
        user: Uuid,
        project: Uuid,
        chat: Uuid,
    }
    async fn fixture() -> Option<Fixture> {
        let db = Database::new().await?;
        let user = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, username, display_name, role, password_hash) VALUES ($1, $2, 'Ana', 'user', 'x')")
            .bind(user)
            .bind(format!("ana_{}", user.simple()))
            .execute(&db.pool)
            .await
            .unwrap();
        let project = Uuid::new_v4();
        sqlx::query("INSERT INTO projects (id, owner_id, name) VALUES ($1, $2, 'Lantern Keep')")
            .bind(project)
            .bind(user)
            .execute(&db.pool)
            .await
            .unwrap();
        let chat = Uuid::new_v4();
        sqlx::query("INSERT INTO conversations (id, owner_id, title, model, project_id) VALUES ($1, $2, 'Levels', 'm', $3)")
            .bind(chat)
            .bind(user)
            .bind(project)
            .execute(&db.pool)
            .await
            .unwrap();
        Some(Fixture {
            db,
            user,
            project,
            chat,
        })
    }
    impl Fixture {
        fn ctx(&self) -> Ctx<'_> {
            Ctx {
                pool: &self.db.pool,
                user: self.user,
                conversation: self.chat,
                project: self.project,
            }
        }
        async fn run(&self, name: &str, args: Value) -> Ran {
            run(&self.ctx(), name, &args.to_string()).await
        }
    }

    #[test]
    fn the_tools_are_named_as_offered() {
        let names: Vec<String> = definitions()
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, NAMES);
        assert!(NAMES.iter().all(|n| is_tool(n)));
        assert!(!is_tool("save_item"));
        assert_eq!(status_of("Needs you"), Some("waiting"));
        assert_eq!(status_of("up-next"), Some("next"));
        assert_eq!(status_of("later"), None);
    }

    #[tokio::test]
    async fn the_assistant_adds_and_works_a_card() {
        let Some(f) = fixture().await else {
            return;
        };
        let added = f
            .run(ADD, json!({"title": "Design level 1", "kind": "Level", "column": "next", "steps": ["Gate", "Hall"]}))
            .await;
        assert!(added.ok, "{}", added.output);
        assert_eq!(added.summary, "Added “Design level 1” to Up next");
        let id = added.card.unwrap();
        let card = super::super::card(&f.db.pool, f.user, f.project, id)
            .await
            .unwrap();
        assert_eq!(
            (card.made_by.as_str(), card.status.as_str()),
            ("assistant", "next")
        );
        assert_eq!(card.source_chat_id, Some(f.chat));
        assert_eq!(card.steps().len(), 2);

        // Started in this chat, with steps and a progress note.
        let short = short(id);
        let started = f
            .run(UPDATE, json!({"card": format!("[{short}]"), "move_to": "now", "progress": "Laying out the hall",
                "steps": [{"text": "Gate", "state": "done"}, {"text": "Hall", "state": "now"}]}))
            .await;
        assert!(started.ok, "{}", started.output);
        assert_eq!(started.summary, "Started “Design level 1”");
        let card = super::super::card(&f.db.pool, f.user, f.project, id)
            .await
            .unwrap();
        assert_eq!(card.status, "doing");
        assert_eq!(card.chat_id, Some(f.chat));
        assert_eq!(card.worker.as_deref(), Some("assistant"));
        assert_eq!(card.progress, "Laying out the hall");

        // A question moves it to Needs you; the person's answer reaches the prompt.
        let asked = f
            .run(UPDATE, json!({"card": short, "question": "Gold or a key for quest 3?", "options": ["Gold", "A key"]}))
            .await;
        assert!(asked.ok, "{}", asked.output);
        assert!(asked.output.contains("stop here"));
        let view = prompt(&f.db.pool, f.user, f.project, f.chat, true)
            .await
            .unwrap();
        assert!(view.contains("## The card this chat is working on"));
        assert!(view.contains("Question asked: Gold or a key for quest 3?"));
        assert!(view.contains("Not answered yet."));
        super::super::change(
            &f.db.pool,
            f.project,
            id,
            Actor::person(f.user),
            Change {
                answer: Some("A key".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let view = prompt(&f.db.pool, f.user, f.project, f.chat, true)
            .await
            .unwrap();
        assert!(view.contains("The person answered: A key"), "{view}");
        assert!(view.contains("in Now"), "{view}");

        let done = f
            .run(UPDATE, json!({"card": short, "move_to": "done"}))
            .await;
        assert_eq!(done.summary, "Marked “Design level 1” done");
        let events: Vec<(String, String)> = sqlx::query_as(
            "SELECT action, actor FROM project_card_events WHERE card_id = $1 ORDER BY created_at, id",
        )
        .bind(id)
        .fetch_all(&f.db.pool)
        .await
        .unwrap();
        let actions: Vec<&str> = events.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(actions, ["made", "picked", "asked", "answered", "done"]);
        assert_eq!(events[3].1, "person");
        assert_eq!(events[4].1, "assistant");
        f.db.close().await;
    }

    #[tokio::test]
    async fn the_model_sees_the_board_and_is_told_what_went_wrong() {
        let Some(f) = fixture().await else {
            return;
        };
        // Nothing to show a model without tools on an empty board.
        assert!(prompt(&f.db.pool, f.user, f.project, f.chat, false)
            .await
            .is_none());
        assert!(prompt(&f.db.pool, f.user, f.project, f.chat, true)
            .await
            .unwrap()
            .contains("The board has no cards yet."));
        for n in 0..10 {
            super::super::make(
                &f.db.pool,
                f.project,
                Actor::person(f.user),
                NewCard {
                    title: format!("Quest {n}"),
                    status: Some("next".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        super::super::make(
            &f.db.pool,
            f.project,
            Actor::person(f.user),
            NewCard {
                title: "A map screen".into(),
                kind: "Feature".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // An idea the person adds shows in the next turn's prompt; long columns are cut.
        let view = prompt(&f.db.pool, f.user, f.project, f.chat, false)
            .await
            .unwrap();
        assert!(view.contains("You cannot change the board"));
        assert!(view.contains("Ideas (1):\n- ["));
        assert!(view.contains("“A map screen” (Feature)"));
        assert!(view.contains("Up next (10):\n1. ["));
        assert!(view.contains("8. ["));
        assert!(!view.contains("9. ["));
        assert!(view.contains("…and 2 more"));
        let listed = f.run(LIST, json!({})).await;
        assert_eq!(listed.summary, "Read the board: 11 open cards");
        assert!(listed.output.contains("10. ["));

        for (name, args, problem) in [
            (UPDATE, json!({"card": "zz"}), "Give the card's id"),
            (
                UPDATE,
                json!({"card": "abcdef12", "progress": "x"}),
                "No card on this board",
            ),
            (ADD, json!({"title": ""}), "Give the card a title"),
            (
                ADD,
                json!({"title": "Later", "column": "done"}),
                "ideas, next or suggested",
            ),
        ] {
            let ran = f.run(name, args).await;
            assert!(!ran.ok);
            assert!(ran.output.contains(problem), "{}", ran.output);
        }
        let id = short(
            sqlx::query_scalar("SELECT id FROM project_cards WHERE title = 'A map screen'")
                .fetch_one(&f.db.pool)
                .await
                .unwrap(),
        );
        let both = f
            .run(
                UPDATE,
                json!({"card": id, "move_to": "done", "question": "Sure?"}),
            )
            .await;
        assert!(both.output.contains("not both"));
        let nothing = f.run(UPDATE, json!({"card": id})).await;
        assert!(nothing.output.contains("Say what to change"));
        let read = f.run(LIST, json!({"card": id})).await;
        assert!(read.output.contains("“A map screen” (Feature) — in Ideas"));
        f.db.close().await;
    }
}
