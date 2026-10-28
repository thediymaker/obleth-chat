//! The export formats, read leniently: a field of an unexpected type is treated as
//! missing rather than failing the whole file.
use chrono::{DateTime, TimeZone, Utc};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

use super::{Node, Raw, Role, Source};

const IMAGE: &str = "[Image not imported]";
const AUDIO: &str = "[Audio not imported]";

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
    chat_messages: Option<Vec<ClaudeMessage>>,
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
                _ => linear(chat.messages.unwrap_or_default(), |m, k, p| {
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
        let messages = self.chat_messages?;
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
    metadata: Option<Value>,
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
    parts: Option<Vec<Value>>,
    #[serde(default, deserialize_with = "text")]
    text: Option<String>,
    #[serde(default)]
    thoughts: Option<Vec<Value>>,
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
        let meta = m.metadata.unwrap_or(Value::Null);
        let role = m.author.and_then(|a| a.role).unwrap_or_default();
        let Some(content) = m.content else {
            return node;
        };
        let kind = content.content_type.as_deref().unwrap_or("text");
        let hidden = [
            "is_visually_hidden_from_conversation",
            "is_thinking_preamble_message",
        ]
        .iter()
        .any(|k| meta.get(k).and_then(Value::as_bool) == Some(true));
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
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|t| t.get("content").and_then(Value::as_str))
                    .filter(|t| !t.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
            // A tool's result shows only when it made an image, as in image generation.
            ("tool", _) => {
                let images = content
                    .parts
                    .iter()
                    .flatten()
                    .filter(|p| part_type(p) == Some("image_asset_pointer"))
                    .count()
                    + meta
                        .pointer("/aggregate_result/messages")
                        .and_then(Value::as_array)
                        .map_or(0, |list| {
                            list.iter()
                                .filter(|m| {
                                    m.get("message_type").and_then(Value::as_str) == Some("image")
                                })
                                .count()
                        });
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
                let fallback = meta
                    .pointer("/model_dil_v2/fallbackMarkdown")
                    .and_then(Value::as_str);
                node.content = match (fallback, kind) {
                    (Some(markdown), _) => markdown.to_owned(),
                    (None, "code") => format!("```\n{}\n```", content.text.unwrap_or_default()),
                    (None, _) => gpt_parts(content.parts.unwrap_or_default()),
                };
                if role == "user" {
                    for file in meta
                        .get("attachments")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let image = file
                            .get("mime_type")
                            .and_then(Value::as_str)
                            .is_some_and(|t| t.starts_with("image/"));
                        if !image {
                            super::join(&mut node.content, &file_line(file.get("name")));
                        }
                    }
                }
                node.model = meta
                    .get("model_slug")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            // Browsing results, code output, memory and custom instructions.
            _ => {}
        }
        node
    }
}

fn part_type(part: &Value) -> Option<&str> {
    part.get("content_type").and_then(Value::as_str)
}

fn gpt_parts(parts: Vec<Value>) -> String {
    let spoken = parts
        .iter()
        .any(|p| part_type(p) == Some("audio_transcription"));
    let mut text = String::new();
    for part in parts {
        match part {
            Value::String(s) => super::join(&mut text, &s),
            p => match part_type(&p) {
                Some("image_asset_pointer") => super::join(&mut text, IMAGE),
                Some("audio_transcription") => super::join(
                    &mut text,
                    p.get("text").and_then(Value::as_str).unwrap_or(""),
                ),
                Some("audio_asset_pointer" | "real_time_user_audio_video_asset_pointer")
                    if !spoken =>
                {
                    super::join(&mut text, AUDIO)
                }
                _ => {}
            },
        }
    }
    text
}

fn file_line(name: Option<&Value>) -> String {
    match name.and_then(Value::as_str).map(str::trim) {
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
    messages: Option<Vec<WebUiMessage>>,
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
    #[serde(default)]
    content: Value,
    #[serde(default, deserialize_with = "text")]
    model: Option<String>,
    #[serde(default, deserialize_with = "stamp")]
    timestamp: Option<DateTime<Utc>>,
    #[serde(default)]
    files: Option<Vec<Value>>,
    #[serde(default)]
    error: Value,
}

impl WebUiMessage {
    fn into_node(self, key: String, parent: Option<String>) -> Node {
        let role = match self.role.as_deref() {
            Some("user") => Some(Role::User),
            Some("assistant") => Some(Role::Assistant),
            _ => None,
        };
        let raw = match &self.content {
            Value::String(s) => s.clone(),
            // A list of parts, as the OpenAI message format allows.
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n"),
            _ => String::new(),
        };
        let (mut content, reasoning) = details(&raw);
        for file in self.files.iter().flatten() {
            let image = file.get("type").and_then(Value::as_str) == Some("image")
                || file
                    .get("content_type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.starts_with("image/"));
            let line = if image {
                IMAGE.to_owned()
            } else {
                file_line(file.get("name"))
            };
            super::join(&mut content, &line);
        }
        let error = match &self.error {
            Value::Null | Value::Bool(false) => None,
            Value::String(s) => Some(s.clone()),
            e => Some(
                e.get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("The reply failed")
                    .to_owned(),
            ),
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
    content: Option<Vec<Value>>,
    #[serde(default, deserialize_with = "stamp")]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    attachments: Option<Vec<Value>>,
    #[serde(default)]
    files: Option<Vec<Value>>,
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
        for part in self.content.iter().flatten() {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => super::join(
                    &mut content,
                    part.get("text").and_then(Value::as_str).unwrap_or(""),
                ),
                Some("thinking") => super::join(
                    &mut reasoning,
                    part.get("thinking").and_then(Value::as_str).unwrap_or(""),
                ),
                _ => {}
            }
        }
        if content.trim().is_empty() {
            content = self.text.unwrap_or_default();
        }
        for file in self.attachments.iter().chain(&self.files).flatten() {
            let name = file.get("file_name");
            let image = name.and_then(Value::as_str).is_some_and(|n| {
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
                    entries.push(entry);
                }
                Ok(Ordered(entries))
            }
        }
        d.deserialize_map(Entries(std::marker::PhantomData))
    }
}

fn text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Value::deserialize(d)?.as_bool().unwrap_or(false))
}

/// Seconds, milliseconds or nanoseconds since 1970, or an RFC 3339 time.
fn stamp<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
    let time = match Value::deserialize(d)? {
        Value::Number(n) => n.as_f64().and_then(epoch),
        Value::String(s) => DateTime::parse_from_rfc3339(s.trim())
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
