//! Chat history brought over from other apps: Open WebUI's "Export chats" file, a
//! ChatGPT data export (the zip, or the conversations.json inside it) and a Claude.ai
//! data export. Each record becomes a message tree that fits what Chat stores: the
//! path to the app's current message is the selected path, edits and regenerated
//! replies become versions, and hidden, system and tool steps fold into the reply
//! they belong to. Files and images are not carried over; a placeholder line marks
//! where they were.
//!
//! Parsing walks the export one record at a time, so only the shaped text of each
//! chat stays in memory, never a second copy of the whole file.
use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use serde::de::{self, Deserializer as _, SeqAccess, Visitor};
use serde::Deserialize;

use crate::error::ApiError;
use crate::store::{ChatArchive, MAX_CONVERSATION_BYTES, MAX_HISTORY, MAX_RESPONSE_BYTES};

mod formats;
mod zip;

/// The largest file accepted, zipped or not.
pub const MAX_IMPORT_BYTES: usize = 200 * 1024 * 1024;
/// Conversations one file may hold.
pub const MAX_IMPORT_CONVERSATIONS: usize = 5_000;
/// Messages and versions kept per conversation, as Chat stores at most.
const MAX_IMPORT_MESSAGES: usize = crate::store::MAX_STORED_MESSAGES;
/// Text kept from one file once shaped, which bounds memory for a zip that inflates.
const MAX_IMPORT_TEXT: usize = 768 * 1024 * 1024;
const SHORTENED: &str = "\n\n[Shortened on import]";
const UNREADABLE: &str = "This file isn't a chat export Chat can read. Choose an export from Chat, Open WebUI, ChatGPT or Claude.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    OpenWebUi,
    ChatGpt,
    Claude,
}
impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenWebUi => "open-webui",
            Self::ChatGpt => "chatgpt",
            Self::Claude => "claude",
        }
    }
}

/// What an uploaded file holds: Chat's own export, or history from another app.
pub enum Upload {
    Archive(ChatArchive),
    Chats(Parsed),
}

/// The chats read from another app's export, and what was left out shaping them.
pub struct Parsed {
    pub source: Source,
    pub chats: Vec<ImportedChat>,
    /// Chats with no message to show.
    pub empty: usize,
    /// The same chat appearing twice in the file.
    pub repeated: usize,
    /// Alternative replies or edits, with what followed them, that did not fit.
    pub alternatives_left_out: usize,
    /// The earliest messages of chats longer than Chat keeps.
    pub messages_left_out: usize,
}

pub struct ImportedChat {
    /// Where it came from, so a second import of the same file skips it.
    pub key: Option<String>,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived: bool,
    pub pinned: bool,
    /// Parents come before their children; the order is each message's ordinal.
    pub messages: Vec<ImportedMessage>,
    /// The last message of the selected path.
    pub leaf: usize,
}

pub struct ImportedMessage {
    pub parent: Option<usize>,
    /// The continuation shown when this message is on the selected path.
    pub selected_child: Option<usize>,
    pub role: Role,
    pub content: String,
    pub reasoning: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    /// The model that wrote a reply, as the other app named it.
    pub model: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}
impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// Reads an uploaded export. CPU-bound: run it off the async workers.
pub fn read(body: &[u8]) -> Result<Upload, ApiError> {
    if body.starts_with(b"PK\x03\x04") || body.starts_with(b"PK\x05\x06") {
        let mut found = Collector::default();
        let entry = zip::conversations(body)?;
        let mut json = serde_json::Deserializer::from_reader(std::io::BufReader::new(entry));
        let result = (&mut json)
            .deserialize_seq(Records(&mut found))
            .and_then(|_| json.end());
        return found.finish(
            result.map_err(|e| ApiError::bad(if e.is_io() { zip::BROKEN } else { UNREADABLE })),
        );
    }
    let body = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body);
    match body.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(b'[') => {
            let mut found = Collector::default();
            let mut json = serde_json::Deserializer::from_slice(body);
            let result = (&mut json)
                .deserialize_seq(Records(&mut found))
                .and_then(|_| json.end());
            found.finish(result.map_err(|_| ApiError::bad(UNREADABLE)))
        }
        Some(b'{') => {
            let value: serde_json::Value =
                serde_json::from_slice(body).map_err(|_| ApiError::bad(UNREADABLE))?;
            if value.get("format").is_some() {
                return serde_json::from_value(value)
                    .map(Upload::Archive)
                    .map_err(|_| {
                        ApiError::bad(
                            "Choose an Obleth Chat v1 export containing 1–50 conversations",
                        )
                    });
            }
            // One conversation saved on its own rather than in a list.
            let mut found = Collector::default();
            let result = formats::Record::deserialize(value).map(|r| found.add(r));
            found.finish(result.map_err(|_| ApiError::bad(UNREADABLE)))
        }
        _ => Err(ApiError::bad(UNREADABLE)),
    }
}

#[derive(Default)]
struct Collector {
    parsed: Option<Parsed>,
    keys: HashSet<String>,
    records: usize,
    text: usize,
    /// Why reading stopped early, in the person's words.
    stop: Option<ApiError>,
}
impl Collector {
    fn add(&mut self, record: formats::Record) {
        if self.stop.is_some() {
            return;
        }
        self.records += 1;
        if self.records > MAX_IMPORT_CONVERSATIONS {
            self.stop = Some(ApiError::bad(
                "This export has more than 5,000 chats. Chat can import up to 5,000 at a time.",
            ));
            return;
        }
        let Some((source, raw)) = record.into_raw() else {
            self.stop = Some(ApiError::bad(UNREADABLE));
            return;
        };
        let parsed = self.parsed.get_or_insert(Parsed {
            source,
            chats: vec![],
            empty: 0,
            repeated: 0,
            alternatives_left_out: 0,
            messages_left_out: 0,
        });
        if let Some(key) = &raw.key {
            if !self.keys.insert(key.clone()) {
                parsed.repeated += 1;
                return;
            }
        }
        let shaped = shape(raw);
        parsed.alternatives_left_out += shaped.alternatives_left_out;
        parsed.messages_left_out += shaped.messages_left_out;
        match shaped.chat {
            Some(chat) => {
                self.text += chat
                    .messages
                    .iter()
                    .map(|m| m.content.len() + m.reasoning.len())
                    .sum::<usize>();
                if self.text > MAX_IMPORT_TEXT {
                    self.stop = Some(ApiError::bad(
                        "This export holds more text than Chat can import at once.",
                    ));
                    return;
                }
                parsed.chats.push(chat);
            }
            None => parsed.empty += 1,
        }
    }

    fn finish(self, result: Result<(), ApiError>) -> Result<Upload, ApiError> {
        if let Some(stop) = self.stop {
            return Err(stop);
        }
        result?;
        match self.parsed {
            Some(parsed) => Ok(Upload::Chats(parsed)),
            None => Err(ApiError::bad("This export has no chats in it.")),
        }
    }
}

/// Hands each record of the top-level list to the collector as soon as it is read.
struct Records<'a>(&'a mut Collector);
impl<'de> Visitor<'de> for Records<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a list of conversations")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(record) = seq.next_element::<formats::Record>()? {
            self.0.add(record);
            if self.0.stop.is_some() {
                return Err(de::Error::custom("stopped"));
            }
        }
        Ok(())
    }
}

/// One conversation as another app stores it: messages linked to their parents.
pub(crate) struct Raw {
    pub key: Option<String>,
    pub title: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub archived: bool,
    pub pinned: bool,
    pub nodes: Vec<Node>,
    /// The message the app was showing last, when it records one.
    pub current: Option<String>,
}

/// A message, or a step that is not one (`role` None): system prompts, tool calls,
/// hidden context. Steps are folded away; their children attach to the nearest message.
pub(crate) struct Node {
    pub key: String,
    pub parent: Option<String>,
    pub role: Option<Role>,
    pub content: String,
    pub reasoning: String,
    pub error: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub model: Option<String>,
}
impl Node {
    fn shown(&self) -> bool {
        self.role.is_some()
            && (!self.content.trim().is_empty()
                || !self.reasoning.trim().is_empty()
                || self.error.is_some())
    }
}

struct Shaped {
    chat: Option<ImportedChat>,
    alternatives_left_out: usize,
    messages_left_out: usize,
}

/// A shown message while the tree is being shaped.
struct Kept {
    node: Node,
    parent: Option<usize>,
    children: Vec<usize>,
    alive: bool,
}

fn shape(raw: Raw) -> Shaped {
    let mut shaped = Shaped {
        chat: None,
        alternatives_left_out: 0,
        messages_left_out: 0,
    };
    let Some((mut kept, roots, current)) = message_tree(raw.nodes, raw.current.as_deref()) else {
        return shaped;
    };
    let mut leaf = current;
    // Fold a run of same-role messages with nothing beside them into one, such as a
    // reply continued after a tool step, or a reasoning step before its answer.
    for k in 0..kept.len() {
        while kept[k].alive && kept[k].children.len() == 1 {
            let c = kept[k].children[0];
            if kept[c].node.role != kept[k].node.role {
                break;
            }
            kept[c].alive = false;
            let grandchildren = std::mem::take(&mut kept[c].children);
            let child = std::mem::replace(&mut kept[c].node, Node::folded());
            for &g in &grandchildren {
                kept[g].parent = Some(k);
            }
            kept[k].children = grandchildren;
            let node = &mut kept[k].node;
            join(&mut node.content, &child.content);
            join(&mut node.reasoning, &child.reasoning);
            node.error = child.error.or(node.error.take());
            node.model = child.model.or(node.model.take());
            if leaf == Some(c) {
                leaf = Some(k);
            }
        }
    }
    // Without a recorded current message, show the most recent end of a branch.
    let leaf = leaf.unwrap_or_else(|| {
        (0..kept.len())
            .filter(|&k| kept[k].alive && kept[k].children.is_empty())
            .max_by_key(|&k| (kept[k].node.created, k))
            .unwrap_or(roots[0])
    });
    let mut full = vec![leaf];
    while let Some(p) = kept[*full.last().unwrap()].parent {
        full.push(p);
    }
    full.reverse();
    for &k in &full {
        clip(&mut kept[k].node);
    }
    // Keep the latest stretch of the path that fits; earlier messages are left out,
    // with any alternatives that branch off them.
    let mut start = full.len();
    let mut bytes = 0;
    while start > 0 && full.len() - start < MAX_HISTORY {
        let size = size(&kept[full[start - 1]].node);
        if bytes + size > MAX_CONVERSATION_BYTES {
            break;
        }
        bytes += size;
        start -= 1;
    }
    shaped.messages_left_out = start;
    let path = &full[start..];
    let mut alternatives: Vec<(usize, usize)> = Vec::new();
    for &r in roots.iter().filter(|&&r| r != full[0]) {
        if start == 0 {
            alternatives.push((r, 0));
        } else {
            shaped.alternatives_left_out += 1;
        }
    }
    for (depth, pair) in full.windows(2).enumerate() {
        for &c in &kept[pair[0]].children {
            if c == pair[1] {
                continue;
            }
            if depth + 1 > start {
                alternatives.push((c, depth + 1 - start));
            } else {
                shaped.alternatives_left_out += 1;
            }
        }
    }
    // Alternatives the person did not end on, kept in order while they fit.
    let mut keep = vec![false; kept.len()];
    for &k in path {
        keep[k] = true;
    }
    let mut count = path.len();
    for (alternative, depth) in alternatives {
        let mut subtree = Vec::new();
        let mut sub_bytes = 0;
        let mut queue = VecDeque::from([(alternative, depth)]);
        while let Some((k, d)) = queue.pop_front() {
            // A version's own path must stay within what Chat shows.
            if d >= MAX_HISTORY {
                continue;
            }
            clip(&mut kept[k].node);
            sub_bytes += size(&kept[k].node);
            subtree.push(k);
            queue.extend(kept[k].children.iter().map(|&c| (c, d + 1)));
        }
        if count + subtree.len() > MAX_IMPORT_MESSAGES || bytes + sub_bytes > MAX_CONVERSATION_BYTES
        {
            shaped.alternatives_left_out += 1;
            continue;
        }
        count += subtree.len();
        bytes += sub_bytes;
        for k in subtree {
            keep[k] = true;
        }
    }
    let first = if start == 0 { roots } else { vec![path[0]] };
    shaped.chat = Some(assemble(
        kept,
        &keep,
        path,
        first,
        Meta {
            key: raw.key,
            title: raw.title,
            created: raw.created,
            updated: raw.updated,
            archived: raw.archived,
            pinned: raw.pinned,
        },
    ));
    shaped
}

/// Shown messages linked to their nearest shown ancestor, with siblings in time
/// order; the roots; and the current message's nearest shown message.
fn message_tree(
    mut nodes: Vec<Node>,
    current: Option<&str>,
) -> Option<(Vec<Kept>, Vec<usize>, Option<usize>)> {
    for node in &mut nodes {
        tidy(node);
    }
    let index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .rev()
        .map(|(i, n)| (n.key.as_str(), i))
        .collect();
    let mut children = vec![Vec::new(); nodes.len()];
    let mut queue = VecDeque::new();
    for (i, n) in nodes.iter().enumerate() {
        match n.parent.as_deref().and_then(|p| index.get(p).copied()) {
            Some(p) if p != i => children[p].push(i),
            _ => queue.push_back(i),
        }
    }
    // Walk down from the roots, so a malformed loop of parents is never followed.
    // `above[i]` is the nearest shown message strictly above a reached node.
    let mut above: Vec<Option<Option<usize>>> = vec![None; nodes.len()];
    for &root in &queue {
        above[root] = Some(None);
    }
    let mut order = Vec::new();
    while let Some(i) = queue.pop_front() {
        order.push(i);
        let here = if nodes[i].shown() {
            Some(i)
        } else {
            above[i].flatten()
        };
        for &c in &children[i] {
            if above[c].is_none() {
                above[c] = Some(here);
                queue.push_back(c);
            }
        }
    }
    let current = current
        .and_then(|k| index.get(k).copied())
        .filter(|&i| above[i].is_some())
        .and_then(|i| {
            if nodes[i].shown() {
                Some(i)
            } else {
                above[i].flatten()
            }
        });
    let mut position = vec![usize::MAX; nodes.len()];
    let shown: Vec<usize> = order.into_iter().filter(|&i| nodes[i].shown()).collect();
    if shown.is_empty() {
        return None;
    }
    for (k, &i) in shown.iter().enumerate() {
        position[i] = k;
    }
    let parents: Vec<Option<usize>> = shown
        .iter()
        .map(|&i| above[i].flatten().map(|a| position[a]))
        .collect();
    let mut taken: Vec<Option<Node>> = nodes.into_iter().map(Some).collect();
    let mut kept: Vec<Kept> = shown
        .iter()
        .zip(parents)
        .map(|(&i, parent)| Kept {
            node: taken[i].take().unwrap(),
            parent,
            children: vec![],
            alive: true,
        })
        .collect();
    let mut roots = Vec::new();
    for k in 0..kept.len() {
        match kept[k].parent {
            Some(p) => kept[p].children.push(k),
            None => roots.push(k),
        }
    }
    sort_by_time(&kept, &mut roots);
    for k in 0..kept.len() {
        let mut list = std::mem::take(&mut kept[k].children);
        sort_by_time(&kept, &mut list);
        kept[k].children = list;
    }
    Some((kept, roots, current.map(|i| position[i])))
}

/// Siblings in the order they were written, when every one says when.
fn sort_by_time(kept: &[Kept], list: &mut [usize]) {
    if list.iter().all(|&k| kept[k].node.created.is_some()) {
        list.sort_by_key(|&k| kept[k].node.created);
    }
}

struct Meta {
    key: Option<String>,
    title: Option<String>,
    created: Option<DateTime<Utc>>,
    updated: Option<DateTime<Utc>>,
    archived: bool,
    pinned: bool,
}

/// Numbers the kept messages breadth first, so parents precede children and each
/// set of versions keeps its order.
fn assemble(
    mut kept: Vec<Kept>,
    keep: &[bool],
    path: &[usize],
    roots: Vec<usize>,
    meta: Meta,
) -> ImportedChat {
    let mut number = vec![usize::MAX; kept.len()];
    let mut order = Vec::new();
    let mut queue: VecDeque<usize> = roots.into_iter().filter(|&r| keep[r]).collect();
    while let Some(k) = queue.pop_front() {
        number[k] = order.len();
        order.push(k);
        queue.extend(kept[k].children.iter().copied().filter(|&c| keep[c]));
    }
    let next: HashMap<usize, usize> = path.windows(2).map(|w| (w[0], w[1])).collect();
    let times: Vec<DateTime<Utc>> = order.iter().filter_map(|&k| kept[k].node.created).collect();
    let now = Utc::now();
    let created = meta
        .created
        .or_else(|| times.iter().min().copied())
        .unwrap_or(now);
    let updated = meta
        .updated
        .or_else(|| times.iter().max().copied())
        .unwrap_or(created)
        .max(created);
    let title = meta
        .title
        .as_deref()
        .map(title_text)
        .filter(|t| !t.is_empty())
        .or_else(|| {
            path.iter()
                .map(|&k| &kept[k].node)
                .find(|n| n.role == Some(Role::User))
                .map(|n| {
                    title_text(
                        n.content
                            .lines()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or(""),
                    )
                })
                .map(|t| t.chars().take(80).collect::<String>())
                .filter(|t| !t.is_empty())
        })
        .unwrap_or_else(|| "Imported chat".into());
    let mut messages = Vec::with_capacity(order.len());
    for &k in &order {
        let selected = next
            .get(&k)
            .copied()
            .or_else(|| kept[k].children.iter().rev().copied().find(|&c| keep[c]));
        let parent = kept[k].parent.filter(|&p| keep[p]).map(|p| number[p]);
        let node = std::mem::replace(&mut kept[k].node, Node::folded());
        let role = node.role.unwrap_or(Role::Assistant);
        messages.push(ImportedMessage {
            parent,
            selected_child: selected.map(|c| number[c]),
            role,
            content: node.content,
            reasoning: if role == Role::Assistant {
                node.reasoning
            } else {
                String::new()
            },
            error: node.error.filter(|_| role == Role::Assistant),
            created_at: node.created.unwrap_or(created),
            model: node.model.filter(|_| role == Role::Assistant),
        });
    }
    ImportedChat {
        key: meta.key,
        title,
        created_at: created,
        updated_at: updated,
        archived: meta.archived,
        pinned: meta.pinned,
        leaf: number[*path.last().unwrap()],
        messages,
    }
}

impl Node {
    fn folded() -> Self {
        Self {
            key: String::new(),
            parent: None,
            role: None,
            content: String::new(),
            reasoning: String::new(),
            error: None,
            created: None,
            model: None,
        }
    }
}

/// Text Postgres and Chat accept: no NUL characters, bounded errors and model names.
fn tidy(node: &mut Node) {
    for text in [&mut node.content, &mut node.reasoning] {
        if text.contains('\0') {
            *text = text.replace('\0', "");
        }
    }
    node.error = node.error.take().map(|e| {
        let e: String = e.replace('\0', "").chars().take(1000).collect();
        if e.trim().is_empty() {
            "The reply failed".into()
        } else {
            e
        }
    });
    node.model = node.model.take().filter(|m| {
        !m.trim().is_empty() && m.chars().count() <= 200 && !m.chars().any(char::is_control)
    });
}

fn join(text: &mut String, more: &str) {
    if more.trim().is_empty() {
        return;
    }
    if !text.trim().is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(more);
}

fn size(node: &Node) -> usize {
    node.content.len() + node.reasoning.len()
}

/// Fits one message into Chat's per-message limit, reasoning first.
fn clip(node: &mut Node) {
    if size(node) <= MAX_RESPONSE_BYTES {
        return;
    }
    let room = MAX_RESPONSE_BYTES.saturating_sub(node.content.len());
    if room < node.reasoning.len() {
        cut(&mut node.reasoning, room);
    }
    if size(node) > MAX_RESPONSE_BYTES {
        node.reasoning.clear();
        cut(&mut node.content, MAX_RESPONSE_BYTES);
    }
}

fn cut(text: &mut String, max: usize) {
    if text.len() <= max {
        return;
    }
    let mut end = max.saturating_sub(SHORTENED.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    if max >= SHORTENED.len() {
        text.push_str(SHORTENED);
    }
}

/// One line of at most 160 characters with no control characters.
fn title_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect()
}
