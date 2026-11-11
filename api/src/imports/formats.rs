//! The export formats, read leniently: a field of an unexpected type is treated as
//! missing rather than failing the whole file. Only the fields Chat uses are kept.
//! The rest of a conversation is skipped as it is read, and what is kept is charged
//! to the conversation's allowance (`budget`), so the memory a file takes follows
//! its text and not its shape.
use std::marker::PhantomData;

use chrono::{DateTime, TimeZone, Utc};
use serde::de::value::MapAccessDeserializer;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Number;

use super::{budget, Node, Raw, Role, Source};

const IMAGE: &str = "[Image not imported]";
const AUDIO: &str = "[Audio not imported]";
const FAILED: &str = "The reply failed";

/// One conversation from any supported app. The field that holds the messages
/// tells the apps apart: ChatGPT's `mapping`, Open WebUI's `chat`, Claude's
/// `chat_messages`.
#[derive(Deserialize)]
pub(super) struct Record {
    #[serde(default, deserialize_with = "text")]
    id: Option<String>,
    #[serde(default, deserialize_with = "text")]
    uuid: Option<String>,
    #[serde(default, deserialize_with = "text")]
    conversation_id: Option<String>,
    #[serde(default, deserialize_with = "text")]
    title: Option<String>,
    #[serde(default, deserialize_with = "text")]
    name: Option<String>,
    #[serde(default, deserialize_with = "stamp")]
    create_time: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "stamp")]
    update_time: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "stamp")]
    created_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "stamp")]
    updated_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "flag")]
    archived: bool,
    #[serde(default, deserialize_with = "flag")]
    is_archived: bool,
    #[serde(default, deserialize_with = "flag")]
    pinned: bool,
    #[serde(default, deserialize_with = "flag")]
    is_starred: bool,
    #[serde(default, deserialize_with = "text")]
    current_node: Option<String>,
    #[serde(default, deserialize_with = "text")]
    current_leaf_message_uuid: Option<String>,
    #[serde(default)]
    mapping: Option<Ordered<GptNode>>,
    #[serde(default)]
    chat: Option<WebUiChat>,
    #[serde(default)]
    chat_messages: Option<Listed<ClaudeMessage>>,
}

impl Record {
    pub(super) fn into_raw(self) -> Option<(Source, Raw)> {
        if let Some(mapping) = self.mapping {
            let id = self.id.or(self.conversation_id);
            return Some((
                Source::ChatGpt,
                Raw {
                    key: id.map(|id| format!("chatgpt:{id}")),
                    title: self.title,
                    created: self.create_time,
                    updated: self.update_time,
                    archived: self.is_archived,
                    pinned: false,
                    nodes: mapping.0.into_iter().map(|(k, n)| n.into_node(k)).collect(),
                    current: self.current_node,
                },
            ));
        }
        if let Some(chat) = self.chat {
            let current = chat.history.as_ref().and_then(|h| h.current_id.clone());
            let nodes = match chat.history {
                Some(WebUiHistory {
                    messages: Some(messages),
                    ..
                }) => messages
                    .0
                    .into_iter()
                    .map(|(k, m)| m.into_node(k, None))
                    .collect(),
                // Early versions kept a plain list of messages.
                _ => linear(chat.messages.map(|l| l.0).unwrap_or_default(), |m, k, p| {
                    m.into_node(k, Some(p))
                }),
            };
            return Some((
                Source::OpenWebUi,
                Raw {
                    key: self.id.map(|id| format!("open-webui:{id}")),
                    title: self.title.or(chat.title),
                    created: self.created_at,
                    updated: self.updated_at,
                    archived: self.archived,
                    pinned: self.pinned,
                    nodes,
                    current,
                },
            ));
        }
        let messages = self.chat_messages?.0;
        // Newer exports link each message to the one it answers; older ones are a list.
        let keys: std::collections::HashSet<&str> =
            messages.iter().filter_map(|m| m.uuid.as_deref()).collect();
        let linked = messages
            .iter()
            .any(|m| m.parent.as_deref().is_some_and(|p| keys.contains(p)));
        let nodes = if linked {
            messages
                .into_iter()
                .enumerate()
                .map(|(i, m)| {
                    let key = m.uuid.clone().unwrap_or_else(|| format!("#{i}"));
                    let parent = m.parent.clone();
                    m.into_node(key, parent)
                })
                .collect()
        } else {
            linear(messages, |m, k, p| m.into_node(k, Some(p)))
        };
        Some((
            Source::Claude,
            Raw {
                key: self.uuid.map(|id| format!("claude:{id}")),
                title: self.name,
                created: self.created_at,
                updated: self.updated_at,
                archived: false,
                pinned: self.is_starred,
                nodes,
                current: self.current_leaf_message_uuid,
            },
        ))
    }
}

/// Links a plain list of messages into a chain.
fn linear<T>(items: Vec<T>, node: impl Fn(T, String, String) -> Node) -> Vec<Node> {
    items
        .into_iter()
        .enumerate()
        .map(|(i, m)| {
            let mut n = node(m, format!("#{i}"), format!("#{}", i.wrapping_sub(1)));
            if i == 0 {
                n.parent = None;
            }
            n
        })
        .collect()
}

#[derive(Deserialize)]
struct GptNode {
    #[serde(default, deserialize_with = "text")]
    parent: Option<String>,
    #[serde(default)]
    message: Option<GptMessage>,
}
#[derive(Deserialize)]
struct GptMessage {
    #[serde(default)]
    author: Option<GptAuthor>,
    #[serde(default)]
    content: Option<GptContent>,
    #[serde(default, deserialize_with = "stamp")]
    create_time: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "text")]
    recipient: Option<String>,
    #[serde(default)]
    metadata: Loose<GptMeta>,
}
/// The parts of a message's metadata Chat reads. The rest, which for a reply that
/// searched the web is most of the conversation, is skipped.
#[derive(Default, Deserialize)]
struct GptMeta {
    #[serde(default, deserialize_with = "flag")]
    is_visually_hidden_from_conversation: bool,
    #[serde(default, deserialize_with = "flag")]
    is_thinking_preamble_message: bool,
    #[serde(default)]
    aggregate_result: Loose<GptAggregate>,
    #[serde(default)]
    model_dil_v2: Loose<GptFallback>,
    #[serde(default)]
    attachments: Listed<Loose<GptAttachment>>,
    #[serde(default, deserialize_with = "string")]
    model_slug: Option<String>,
}
#[derive(Default, Deserialize)]
struct GptAggregate {
    #[serde(default)]
    messages: Listed<Loose<GptAggregateMessage>>,
}
#[derive(Default, Deserialize)]
struct GptAggregateMessage {
    #[serde(default, deserialize_with = "string")]
    message_type: Option<String>,
}
#[derive(Default, Deserialize)]
struct GptFallback {
    #[serde(default, rename = "fallbackMarkdown", deserialize_with = "string")]
    markdown: Option<String>,
}
#[derive(Default, Deserialize)]
struct GptAttachment {
    #[serde(default, deserialize_with = "string")]
    mime_type: Option<String>,
    #[serde(default, deserialize_with = "string")]
    name: Option<String>,
}
#[derive(Deserialize)]
struct GptAuthor {
    #[serde(default, deserialize_with = "text")]
    role: Option<String>,
}
#[derive(Deserialize)]
struct GptContent {
    #[serde(default, deserialize_with = "text")]
    content_type: Option<String>,
    #[serde(default)]
    parts: Listed<GptPart>,
    #[serde(default, deserialize_with = "text")]
    text: Option<String>,
    #[serde(default)]
    thoughts: Listed<Loose<GptThought>>,
}
/// Text, or an object naming what it stands for: an image, audio, a transcription.
type GptPart = Any<GptPartFields, IgnoredAny>;
#[derive(Deserialize)]
struct GptPartFields {
    #[serde(default, deserialize_with = "string")]
    content_type: Option<String>,
    #[serde(default, deserialize_with = "string")]
    text: Option<String>,
}
#[derive(Default, Deserialize)]
struct GptThought {
    #[serde(default, deserialize_with = "string")]
    content: Option<String>,
}

impl GptNode {
    fn into_node(self, key: String) -> Node {
        let mut node = Node {
            key,
            parent: self.parent,
            ..Node::folded()
        };
        let Some(m) = self.message else {
            return node;
        };
        node.created = m.create_time;
        let meta = m.metadata.0;
        let role = m.author.and_then(|a| a.role).unwrap_or_default();
        let Some(content) = m.content else {
            return node;
        };
        let kind = content.content_type.as_deref().unwrap_or("text");
        let hidden = meta.is_visually_hidden_from_conversation || meta.is_thinking_preamble_message;
        // Messages addressed to a tool (a search, code to run, an image request) are
        // steps, not part of the transcript.
        let to_tool = m.recipient.as_deref().is_some_and(|r| r != "all");
        if hidden || to_tool || role == "system" {
            return node;
        }
        match (role.as_str(), kind) {
            ("assistant", "thoughts") => {
                node.role = Some(Role::Assistant);
                node.reasoning = content
                    .thoughts
                    .0
                    .iter()
                    .filter_map(|t| t.0.content.as_deref())
                    .filter(|t| !t.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
            // A tool's result shows only when it made an image, as in image generation.
            ("tool", _) => {
                let images = content
                    .parts
                    .0
                    .iter()
                    .filter(|p| part_type(p) == Some("image_asset_pointer"))
                    .count()
                    + meta
                        .aggregate_result
                        .0
                        .messages
                        .0
                        .iter()
                        .filter(|m| m.0.message_type.as_deref() == Some("image"))
                        .count();
                if images > 0 {
                    node.role = Some(Role::Assistant);
                    node.content = vec![IMAGE; images].join("\n");
                }
            }
            ("user" | "assistant", "text" | "multimodal_text" | "code") => {
                node.role = Some(if role == "user" {
                    Role::User
                } else {
                    Role::Assistant
                });
                node.content = match (meta.model_dil_v2.0.markdown, kind) {
                    (Some(markdown), _) => markdown,
                    (None, "code") => format!("```\n{}\n```", content.text.unwrap_or_default()),
                    (None, _) => gpt_parts(content.parts.0),
                };
                if role == "user" {
                    for file in &meta.attachments.0 {
                        let image = file
                            .0
                            .mime_type
                            .as_deref()
                            .is_some_and(|t| t.starts_with("image/"));
                        if !image {
                            super::join(&mut node.content, &file_line(file.0.name.as_deref()));
                        }
                    }
                }
                node.model = meta.model_slug;
            }
            // Browsing results, code output, memory and custom instructions.
            _ => {}
        }
        node
    }
}

fn part_type(part: &GptPart) -> Option<&str> {
    match part {
        Any::Object(fields) => fields.content_type.as_deref(),
        _ => None,
    }
}

fn gpt_parts(parts: Vec<GptPart>) -> String {
    let spoken = parts
        .iter()
        .any(|p| part_type(p) == Some("audio_transcription"));
    let mut text = String::new();
    for part in parts {
        match part {
            Any::Text(s) => super::join(&mut text, &s),
            Any::Object(p) => match p.content_type.as_deref() {
                Some("image_asset_pointer") => super::join(&mut text, IMAGE),
                Some("audio_transcription") => {
                    super::join(&mut text, p.text.as_deref().unwrap_or(""))
                }
                Some("audio_asset_pointer" | "real_time_user_audio_video_asset_pointer")
                    if !spoken =>
                {
                    super::join(&mut text, AUDIO)
                }
                _ => {}
            },
            _ => {}
        }
    }
    text
}

fn file_line(name: Option<&str>) -> String {
    match name.map(str::trim) {
        Some(name) if !name.is_empty() => {
            let name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
            format!("[File not imported: {name}]")
        }
        _ => "[File not imported]".into(),
    }
}

#[derive(Deserialize)]
struct WebUiChat {
    #[serde(default, deserialize_with = "text")]
    title: Option<String>,
    #[serde(default)]
    history: Option<WebUiHistory>,
    #[serde(default)]
    messages: Option<Listed<WebUiMessage>>,
}
#[derive(Deserialize)]
struct WebUiHistory {
    #[serde(default, rename = "currentId", deserialize_with = "text")]
    current_id: Option<String>,
    #[serde(default)]
    messages: Option<Ordered<WebUiMessage>>,
}
#[derive(Deserialize)]
struct WebUiMessage {
    #[serde(default, rename = "parentId", deserialize_with = "text")]
    parent_id: Option<String>,
    #[serde(default, deserialize_with = "text")]
    role: Option<String>,
    /// Text, or a list of parts as the OpenAI message format allows.
    #[serde(default)]
    content: Any<IgnoredAny, Loose<TextPart>>,
    #[serde(default, deserialize_with = "text")]
    model: Option<String>,
    #[serde(default, deserialize_with = "stamp")]
    timestamp: Option<DateTime<Utc>>,
    /// Open WebUI keeps a document's whole text here; only what names it is read.
    #[serde(default)]
    files: Listed<Loose<WebUiFile>>,
    #[serde(default)]
    error: Any<Failure, IgnoredAny>,
}
#[derive(Default, Deserialize)]
struct TextPart {
    #[serde(default, deserialize_with = "string")]
    text: Option<String>,
}
#[derive(Default, Deserialize)]
struct WebUiFile {
    #[serde(default, rename = "type", deserialize_with = "string")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "string")]
    content_type: Option<String>,
    #[serde(default, deserialize_with = "string")]
    name: Option<String>,
}
#[derive(Deserialize)]
struct Failure {
    #[serde(default, deserialize_with = "string")]
    content: Option<String>,
}

impl WebUiMessage {
    fn into_node(self, key: String, parent: Option<String>) -> Node {
        let role = match self.role.as_deref() {
            Some("user") => Some(Role::User),
            Some("assistant") => Some(Role::Assistant),
            _ => None,
        };
        let raw = match self.content {
            Any::Text(s) => s,
            Any::List(parts) => parts
                .iter()
                .filter_map(|p| p.0.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n\n"),
            _ => String::new(),
        };
        let (mut content, reasoning) = details(&raw);
        for file in &self.files.0 {
            let image = file.0.kind.as_deref() == Some("image")
                || file
                    .0
                    .content_type
                    .as_deref()
                    .is_some_and(|t| t.starts_with("image/"));
            let line = if image {
                IMAGE.to_owned()
            } else {
                file_line(file.0.name.as_deref())
            };
            super::join(&mut content, &line);
        }
        let error = match self.error {
            Any::Null | Any::Bool(false) => None,
            Any::Text(s) => Some(s),
            Any::Object(e) => Some(e.content.unwrap_or_else(|| FAILED.into())),
            _ => Some(FAILED.into()),
        };
        Node {
            key,
            parent: parent.or(self.parent_id),
            role,
            content,
            reasoning,
            error,
            created: self.timestamp,
            model: self.model,
        }
    }
}

/// Open WebUI writes reasoning and tool calls into a reply as `<details type="…">`
/// blocks. Reasoning moves to the reply's reasoning; other steps are dropped.
pub(super) fn details(content: &str) -> (String, String) {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut rest = content;
    while let Some(start) = rest.find("<details") {
        let open_end = match rest[start..].find('>') {
            Some(e) => start + e + 1,
            None => break,
        };
        let tag = &rest[start..open_end];
        let kind = tag
            .split("type=\"")
            .nth(1)
            .and_then(|t| t.split('"').next());
        let Some(kind) = kind else {
            text.push_str(&rest[..open_end]);
            rest = &rest[open_end..];
            continue;
        };
        // The matching close, allowing details nested inside.
        let mut depth = 1;
        let mut at = open_end;
        let mut close = None;
        while depth > 0 {
            let next_open = rest[at..].find("<details").map(|i| at + i);
            let Some(next_close) = rest[at..].find("</details>").map(|i| at + i) else {
                break;
            };
            match next_open {
                Some(o) if o < next_close => {
                    depth += 1;
                    at = o + 8;
                }
                _ => {
                    depth -= 1;
                    at = next_close + 10;
                    if depth == 0 {
                        close = Some(next_close);
                    }
                }
            }
        }
        let Some(close) = close else {
            break;
        };
        text.push_str(&rest[..start]);
        if kind == "reasoning" {
            let inner = &rest[open_end..close];
            let inner = match inner.find("</summary>") {
                Some(i) => &inner[i + 10..],
                None => inner,
            };
            let thought = inner
                .lines()
                .map(|l| l.strip_prefix("> ").or(l.strip_prefix('>')).unwrap_or(l))
                .collect::<Vec<_>>()
                .join("\n");
            super::join(&mut reasoning, thought.trim());
        }
        rest = &rest[close + 10..];
    }
    text.push_str(rest);
    (text.trim().to_owned(), reasoning)
}

#[derive(Deserialize)]
struct ClaudeMessage {
    #[serde(default, deserialize_with = "text")]
    uuid: Option<String>,
    #[serde(default, rename = "parent_message_uuid", deserialize_with = "text")]
    parent: Option<String>,
    #[serde(default, deserialize_with = "text")]
    sender: Option<String>,
    #[serde(default, deserialize_with = "text")]
    text: Option<String>,
    #[serde(default)]
    content: Listed<Loose<ClaudePart>>,
    #[serde(default, deserialize_with = "stamp")]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    attachments: Listed<Loose<ClaudeFile>>,
    #[serde(default)]
    files: Listed<Loose<ClaudeFile>>,
}
#[derive(Default, Deserialize)]
struct ClaudePart {
    #[serde(default, rename = "type", deserialize_with = "string")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "string")]
    text: Option<String>,
    #[serde(default, deserialize_with = "string")]
    thinking: Option<String>,
}
#[derive(Default, Deserialize)]
struct ClaudeFile {
    #[serde(default, deserialize_with = "string")]
    file_name: Option<String>,
}

impl ClaudeMessage {
    fn into_node(self, key: String, parent: Option<String>) -> Node {
        let role = match self.sender.as_deref() {
            Some("human") => Some(Role::User),
            Some("assistant") => Some(Role::Assistant),
            _ => None,
        };
        let mut content = String::new();
        let mut reasoning = String::new();
        // Structured parts when present; tool calls and their results are steps.
        for part in &self.content.0 {
            match part.0.kind.as_deref() {
                Some("text") => super::join(&mut content, part.0.text.as_deref().unwrap_or("")),
                Some("thinking") => {
                    super::join(&mut reasoning, part.0.thinking.as_deref().unwrap_or(""))
                }
                _ => {}
            }
        }
        if content.trim().is_empty() {
            content = self.text.unwrap_or_default();
        }
        for file in self.attachments.0.iter().chain(&self.files.0) {
            let name = file.0.file_name.as_deref();
            let image = name.is_some_and(|n| {
                let n = n.to_ascii_lowercase();
                [".png", ".jpg", ".jpeg", ".gif", ".webp", ".heic"]
                    .iter()
                    .any(|e| n.ends_with(e))
            });
            let line = if image {
                IMAGE.to_owned()
            } else {
                file_line(name)
            };
            super::join(&mut content, &line);
        }
        Node {
            key,
            parent,
            role,
            content,
            reasoning,
            error: None,
            created: self.created_at,
            model: None,
        }
    }
}

/// A JSON object read in its written order; ChatGPT and Open WebUI keep messages
/// in objects keyed by id.
pub(super) struct Ordered<T>(pub Vec<(String, T)>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Ordered<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Entries<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Entries<T> {
            type Value = Ordered<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, T>()? {
                    budget::slot::<(String, T), A::Error>()?;
                    budget::charge::<A::Error>(entry.0.len())?;
                    entries.push(entry);
                }
                Ok(Ordered(entries))
            }
        }
        d.deserialize_map(Entries(std::marker::PhantomData))
    }
}

/// A JSON value as far as Chat looks at it: an object read as `O`, a list of `I`, or
/// a plain value. `IgnoredAny` in either place skips that kind without keeping it.
#[derive(Default)]
enum Any<O, I> {
    #[default]
    Null,
    Bool(bool),
    Number(Number),
    Text(String),
    Object(O),
    List(Vec<I>),
}
impl<'de, O: Deserialize<'de>, I: Deserialize<'de>> Deserialize<'de> for Any<O, I> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Kind<O, I>(PhantomData<(O, I)>);
        impl<'de, O: Deserialize<'de>, I: Deserialize<'de>> Visitor<'de> for Kind<O, I> {
            type Value = Any<O, I>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Any::Null)
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(Any::Null)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
                d.deserialize_any(self)
            }
            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
                Ok(Any::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(Any::Number(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(Any::Number(v.into()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
                Ok(Number::from_f64(v).map_or(Any::Null, Any::Number))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                budget::charge::<E>(v.len())?;
                Ok(Any::Text(v.to_owned()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                budget::charge::<E>(v.len())?;
                Ok(Any::Text(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut list = Vec::new();
                while let Some(item) = seq.next_element::<I>()? {
                    budget::slot::<I, A::Error>()?;
                    list.push(item);
                }
                Ok(Any::List(list))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                O::deserialize(MapAccessDeserializer::new(map)).map(Any::Object)
            }
        }
        d.deserialize_any(Kind(PhantomData))
    }
}

/// An object with the fields `T` names. Anything else in its place reads as empty.
#[derive(Default)]
struct Loose<T>(T);
impl<'de, T: Deserialize<'de> + Default> Deserialize<'de> for Loose<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Any::<T, IgnoredAny>::deserialize(d)? {
            Any::Object(fields) => Self(fields),
            _ => Self::default(),
        })
    }
}

/// A list. Anything else in its place reads as an empty one.
pub(super) struct Listed<T>(pub Vec<T>);
impl<T> Default for Listed<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Listed<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Any::<IgnoredAny, T>::deserialize(d)? {
            Any::List(items) => Self(items),
            _ => Self::default(),
        })
    }
}

/// A value that is not an object or a list; those are skipped.
type Plain = Any<IgnoredAny, IgnoredAny>;

/// Text, or a number as text, as ids are written either way.
fn text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Plain::deserialize(d)? {
        Any::Text(s) => Some(s),
        Any::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Plain::deserialize(d)? {
        Any::Text(s) => Some(s),
        _ => None,
    })
}

fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(matches!(Plain::deserialize(d)?, Any::Bool(true)))
}

/// Seconds, milliseconds or nanoseconds since 1970, or an RFC 3339 time.
fn stamp<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
    let time = match Plain::deserialize(d)? {
        Any::Number(n) => n.as_f64().and_then(epoch),
        Any::Text(s) => DateTime::parse_from_rfc3339(s.trim())
            .map(|t| t.with_timezone(&Utc))
            .ok()
            .or_else(|| s.trim().parse::<f64>().ok().and_then(epoch)),
        _ => None,
    };
    // Anything before 2000 or far ahead is a unit mix-up, not a real time.
    Ok(time.filter(|t| t.timestamp() > 946_684_800 && t.timestamp() < 4_102_444_800))
}

fn epoch(value: f64) -> Option<DateTime<Utc>> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let seconds = match value {
        v if v > 1e17 => v / 1e9,
        v if v > 1e14 => v / 1e6,
        v if v > 1e11 => v / 1e3,
        v => v,
    };
    Utc.timestamp_millis_opt((seconds * 1000.0) as i64).single()
}
