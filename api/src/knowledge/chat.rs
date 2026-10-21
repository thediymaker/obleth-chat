//! Knowledge in a chat: passages found for the person's message go into the system
//! prompt with numbers the model cites as [n], and models that call tools can search
//! again with `search_knowledge`. Every source shown is recorded on the reply as a
//! `sources` activity entry so the page can link each citation to its page.

use super::{excerpt, Hit, Knowledge};
use crate::{auth::Identity, gateway::Gateway};
use serde_json::{json, Value};
use uuid::Uuid;

pub const TOOL: &str = "search_knowledge";
pub const SAVE_TOOL: &str = "save_item";
/// Saved items a model search adds next to collection passages.
const SAVED_HITS: usize = 3;
const FIRST: usize = 6;
const MORE: usize = 5;
const MAX_SOURCES: usize = 30;
/// Searches the model may make itself in one reply, after the first one Chat runs.
const MAX_SEARCHES: usize = 4;

pub struct Turn {
    pub collections: Vec<Uuid>,
    searches: usize,
    names: Vec<String>,
    sources: Vec<Hit>,
    queries: Vec<String>,
    pub tool: bool,
    /// The person has saved items a search can find.
    saved: bool,
    /// The model surely calls tools, so it gets save_item and saved-item search in
    /// any chat, not only one with collections.
    pub library: bool,
    /// The chat's project, whose collections and saved items it also searches.
    project: Option<Uuid>,
}

impl Knowledge {
    /// Searches the conversation's collections for the person's message. A model that
    /// calls tools also gets save_item, and search_knowledge over saved items, in any
    /// chat. None when there is nothing to offer.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare(
        &self,
        gateway: &Gateway,
        user: &Identity,
        conversation: Uuid,
        messages: &[(String, String)],
        tools: bool,
        library: bool,
        project: Option<&crate::projects::Context>,
    ) -> Option<Turn> {
        let mut collections = super::for_conversation(&self.pool, user.id, conversation)
            .await
            .unwrap_or_default();
        for id in project
            .map(|p| p.collections.as_slice())
            .unwrap_or_default()
        {
            if !collections.contains(id) {
                collections.push(*id);
            }
        }
        if collections.is_empty() && !library {
            return None;
        }
        let saved = library && self.has_saved(user.id, project.map(|p| p.id)).await;
        let names = self.names(&collections).await;
        let query = if collections.is_empty() {
            String::new()
        } else {
            question(messages)
        };
        let sources = match self
            .search(gateway, user, &collections, &query, FIRST)
            .await
        {
            Ok(hits) => hits,
            Err(error) => {
                tracing::warn!("knowledge search failed: {}", error.1);
                vec![]
            }
        };
        Some(Turn {
            collections,
            searches: 0,
            names,
            sources,
            queries: if query.is_empty() {
                vec![]
            } else {
                vec![query]
            },
            tool: tools,
            saved,
            library,
            project: project.map(|p| p.id),
        })
    }

    /// Runs a `search_knowledge` call and returns what the model reads.
    pub async fn call(
        &self,
        gateway: &Gateway,
        user: &Identity,
        turn: &mut Turn,
        arguments: &str,
    ) -> (bool, String, String) {
        let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
        let query = args["query"].as_str().unwrap_or("").trim().to_owned();
        if query.is_empty() {
            return (
                false,
                "Give a query to search for.".into(),
                "No query".into(),
            );
        }
        if turn.searches >= MAX_SEARCHES {
            return (
                true,
                format!("That is {MAX_SEARCHES} searches, the most for one answer. Answer now from the passages you have, and say plainly what they do not cover."),
                "Search limit reached".into(),
            );
        }
        turn.searches += 1;
        let found = async {
            let mut hits = self
                .search(gateway, user, &turn.collections, &query, MORE)
                .await?;
            if turn.saved {
                hits.extend(
                    self.search_saved(gateway, user, &query, turn.project, SAVED_HITS)
                        .await?,
                );
            }
            Ok::<_, crate::error::ApiError>(hits)
        };
        let hits = match found.await {
            Ok(hits) => hits,
            Err(error) => {
                return (
                    false,
                    format!("The search failed: {}", error.1),
                    "Search failed".into(),
                )
            }
        };
        turn.queries.push(query.chars().take(200).collect());
        if hits.is_empty() {
            return (
                true,
                "No passages matched. Try other words, or answer without the documents and say so."
                    .into(),
                "No passages".into(),
            );
        }
        let before = turn.sources.len();
        let mut out = String::from("Passages found (cite them by number):\n");
        let mut seen = vec![];
        for hit in hits {
            let n = turn.number(hit);
            if n <= before {
                seen.push(n);
                continue;
            }
            let hit = &turn.sources[n - 1];
            out.push_str(&format!("\n[{n}] {}\n{}\n", label(hit), hit.content));
        }
        let new = turn.sources.len() - before;
        if new == 0 {
            let list = seen
                .iter()
                .map(|n| format!("[{n}]"))
                .collect::<Vec<_>>()
                .join(", ");
            return (
                true,
                format!("No new passages: this search found only {list}, which you already have. Searching again with similar words will not find more; answer from what you have."),
                "Nothing new".into(),
            );
        }
        if !seen.is_empty() {
            let list = seen
                .iter()
                .map(|n| format!("[{n}]"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("\nAlso matched, already above: {list}.\n"));
        }
        (true, out, format!("{new} new"))
    }
}

impl Turn {
    /// Whether the model gets search_knowledge.
    pub fn searchable(&self) -> bool {
        (self.tool && !self.collections.is_empty()) || (self.library && self.saved)
    }
    /// Whether the reply shows a Sources card from the start.
    pub fn has_collections(&self) -> bool {
        !self.collections.is_empty()
    }
    /// The number a passage is cited by, reusing it when the same passage comes back.
    fn number(&mut self, hit: Hit) -> usize {
        if let Some(i) = self.sources.iter().position(|s| {
            s.document == hit.document && s.page == hit.page && s.content == hit.content
        }) {
            return i + 1;
        }
        if self.sources.len() >= MAX_SOURCES {
            return self.sources.len();
        }
        self.sources.push(hit);
        self.sources.len()
    }

    /// The system prompt section with the passages found for the message.
    pub fn prompt(&self) -> String {
        let mut text = String::new();
        if self.library {
            text.push_str(&format!("# The person's library\nWhen the person asks you to remember, keep or save something (a function, a fact, a link, a decision), call {SAVE_TOOL} with a clear title, the content and a few tags. Only save what they ask for."));
            if self.saved {
                text.push_str(&format!(" They also keep saved items from earlier work; when they refer to something they saved, or to earlier work you cannot see, call {TOOL} to find it."));
            }
        }
        if self.collections.is_empty() {
            return text;
        }
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!(
            "# From the person's documents\nThis chat searches the person's knowledge collections: {}. Use these passages when they help answer, and cite each one you use with its number in square brackets right after the sentence that relies on it, like [2]. Cite only passages you actually used, and never invent a number. If the passages do not answer the question, say that before answering from general knowledge.",
            self.names.join(", ")
        ));
        if self.tool {
            text.push_str(&format!(" To look for more, call {TOOL} with a short query; its passages continue the numbering."));
        }
        if self.sources.is_empty() {
            text.push_str("\n\nNo passages matched the person's message.");
        }
        for (i, hit) in self.sources.iter().enumerate() {
            text.push_str(&format!("\n\n[{}] {}\n{}", i + 1, label(hit), hit.content));
        }
        text
    }

    /// The activity entry the page shows: what was searched and every numbered source.
    pub fn entry(&self) -> Value {
        let mut names = self.names.clone();
        if names.is_empty() || self.sources.iter().any(|h| h.saved) {
            names.push("your saved items".into());
        }
        let summary = match self.queries.first() {
            Some(query) => format!("Searched {} for “{}”", names.join(", "), excerpt(query, 80)),
            None => format!("Searched {}", names.join(", ")),
        };
        json!({"id":"knowledge","kind":"sources","tool":TOOL,"summary":summary,"status":"completed",
            "queries":self.queries,"collections":names,
            "items":self.sources.iter().enumerate().map(|(i, h)| h.json(i + 1)).collect::<Vec<_>>()})
    }
}

pub fn definition() -> Value {
    json!({"type":"function","function":{"name":TOOL,
        "description":"Search the person's attached knowledge collections for passages about a topic. Use short, specific queries; call again with different words if the first search misses. Results are numbered so you can cite them as [n].",
        "parameters":{"type":"object","properties":{"query":{"type":"string","description":"What to look for, in a few words or a short question"}},"required":["query"],"additionalProperties":false}}})
}

/// The save_item tool: what the model calls when the person says "remember this".
pub fn save_definition() -> Value {
    json!({"type":"function","function":{"name":SAVE_TOOL,
        "description":"Save something to the person's library when they ask you to remember or keep it: a function, a fact, a link or a decision. They can find it later in Library, and chats can search it.",
        "parameters":{"type":"object","properties":{
            "title":{"type":"string","description":"A short title that says what it is"},
            "kind":{"type":"string","enum":["code","note","link","excerpt"],"description":"code for a function or command, note for a fact or decision, link for a web address, excerpt for a passage of your answer"},
            "content":{"type":"string","description":"What to keep, complete: the full code, the fact, or the passage"},
            "language":{"type":"string","description":"For code: the language, such as python"},
            "url":{"type":"string","description":"For a link: the address"},
            "tags":{"type":"array","items":{"type":"string"},"description":"Two to four short lowercase tags, such as research or python"}},
        "required":["title","kind","content"],"additionalProperties":false}}})
}

impl Knowledge {
    /// Runs a save_item call. Returns whether it worked, what the model reads, and the
    /// activity entry the page shows as a card with Undo.
    #[allow(clippy::too_many_arguments)]
    pub async fn save_call(
        &self,
        gateway: &Gateway,
        user: &Identity,
        turn: &Turn,
        conversation: Uuid,
        message: Uuid,
        call_id: &str,
        arguments: &str,
    ) -> (bool, String, Value) {
        let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
        let text = |key: &str| args[key].as_str().map(str::to_owned);
        let tags: Vec<String> = args["tags"]
            .as_array()
            .map(|t| {
                t.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let input = super::saved::Input {
            kind: text("kind"),
            title: text("title"),
            body: text("content"),
            language: text("language"),
            url: text("url"),
            tags: Some(tags),
            searchable: Some(true),
            source_conversation_id: Some(conversation),
            source_message_id: Some(message),
            project_id: Some(turn.project),
            pinned: None,
        };
        match self.save(Some(gateway), user, input).await {
            Ok(item) => {
                let title = item["title"].as_str().unwrap_or("").to_owned();
                (
                    true,
                    format!("Saved “{title}” to the person's library. Tell them in a few words; they can undo it."),
                    json!({"id":call_id,"kind":"saved","tool":SAVE_TOOL,"status":"completed","summary":format!("Saved “{title}”"),
                        "savedId":item["id"],"title":title,"tags":item["tags"],"itemKind":item["kind"]}),
                )
            }
            Err(error) => (
                false,
                format!("Could not save it: {}", error.1),
                json!({"id":call_id,"kind":"saved","tool":SAVE_TOOL,"status":"error","summary":"Could not save","detail":error.1}),
            ),
        }
    }
}

fn label(hit: &Hit) -> String {
    if hit.saved {
        return format!("saved: {}", hit.name);
    }
    match hit.page {
        Some(page) => format!("{}, page {page}", hit.name),
        None => hit.name.clone(),
    }
}

/// What to search for: the latest message, with the one before it when the latest is
/// a short follow-up such as "why?" or "and the second one?".
fn question(messages: &[(String, String)]) -> String {
    let mut users = messages
        .iter()
        .rev()
        .filter(|(role, _)| role == "user")
        .map(|(_, c)| c.trim());
    let last = users.next().unwrap_or("");
    let text = if last.chars().count() < 60 {
        match users.next() {
            Some(previous) => format!("{previous}\n{last}"),
            None => last.to_owned(),
        }
    } else {
        last.to_owned()
    };
    text.chars().take(1500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_follow_ups_search_with_the_question_before() {
        let messages = vec![
            (
                "user".to_string(),
                "How does a free expansion change the entropy of an ideal gas?".to_string(),
            ),
            ("assistant".to_string(), "It rises.".to_string()),
            ("user".to_string(), "why?".to_string()),
        ];
        assert_eq!(
            question(&messages),
            "How does a free expansion change the entropy of an ideal gas?\nwhy?"
        );
        assert_eq!(question(&messages[..1]), messages[0].1);
    }

    #[test]
    fn repeated_passages_keep_their_number() {
        let hit = |page| Hit {
            document: Uuid::nil(),
            collection: Uuid::nil(),
            name: "Notes.pdf".into(),
            page: Some(page),
            content: format!("text {page}"),
            matched: "meaning",
            saved: false,
        };
        let mut turn = Turn {
            collections: vec![Uuid::nil()],
            names: vec!["Notes".into()],
            searches: 0,
            sources: vec![hit(1)],
            queries: vec!["q".into()],
            tool: true,
            saved: false,
            library: false,
            project: None,
        };
        assert_eq!(turn.number(hit(2)), 2);
        assert_eq!(turn.number(hit(1)), 1);
        assert!(turn.prompt().contains("[2] Notes.pdf, page 2\ntext 2"));
        assert_eq!(turn.entry()["items"].as_array().unwrap().len(), 2);
    }
}
