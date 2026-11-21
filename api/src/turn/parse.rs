//! Tool calls as a model sends them: streamed fragments, calls written into the
//! reply's text, and the file a streaming write is producing.

use serde_json::{json, Value};

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Gives each call an id no earlier call in the turn used. Calls read from the
/// reply's text, and calls a model sends without an id, are numbered from zero in
/// every step; the step's activity, its approval and its result are all found by
/// the id, so a repeated one would let a later call pass for an earlier one.
pub(crate) fn unique_call_ids(
    calls: &mut [ToolCall],
    used: &mut std::collections::HashSet<String>,
    step: usize,
) {
    for (position, call) in calls.iter_mut().enumerate() {
        if used.insert(call.id.clone()) {
            continue;
        }
        let mut id = format!("call_s{step}_{position}");
        let mut again = 0;
        while !used.insert(id.clone()) {
            again += 1;
            id = format!("call_s{step}_{position}_{again}");
        }
        call.id = id;
    }
}

/// The file a streaming write_file or edit_file call is producing: its path and the
/// last lines typed so far.
pub(crate) fn draft_of(call: &ToolCall) -> Option<Value> {
    let field = match call.name.as_str() {
        "write_file" => "content",
        "edit_file" => "new_text",
        _ => return None,
    };
    let path = partial_string(&call.arguments, "path").unwrap_or_default();
    let text = partial_string(&call.arguments, field)?;
    let lines = text.lines().count();
    let tail: Vec<&str> = text.lines().rev().take(8).collect();
    let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    Some(
        json!({"id":call.id,"tool":call.name,"path":path.chars().take(200).collect::<String>(),"lines":lines,"tail":tail.chars().rev().take(1200).collect::<String>().chars().rev().collect::<String>()}),
    )
}

/// The value of `"key": "…"` in JSON that may still be arriving, unescaped as far as
/// it goes.
fn partial_string(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => {}
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    Some(out)
}

/// Merges streamed tool-call fragments, which arrive keyed by index.
pub(crate) fn accumulate_tool_calls(
    calls: &mut Vec<ToolCall>,
    deltas: &[Value],
) -> Result<(), String> {
    for (position, delta) in deltas.iter().enumerate() {
        let index = delta
            .get("index")
            .and_then(Value::as_u64)
            .map_or(position, |i| i as usize);
        if index >= 16 {
            return Err("The model requested too many tools at once".into());
        }
        while calls.len() <= index {
            calls.push(ToolCall {
                id: format!("call_{}", calls.len()),
                name: String::new(),
                arguments: String::new(),
            });
        }
        let call = &mut calls[index];
        if let Some(id) = delta
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 128)
        {
            call.id = id.to_owned();
        }
        if let Some(name) = delta.pointer("/function/name").and_then(Value::as_str) {
            call.name.push_str(name);
        }
        if let Some(arguments) = delta.pointer("/function/arguments").and_then(Value::as_str) {
            call.arguments.push_str(arguments);
        }
        if call.name.len() > 64 || call.arguments.len() > 2 * 1024 * 1024 {
            return Err("The model sent an oversized tool call".into());
        }
    }
    Ok(())
}

/// Tool calls a model wrote as text because its server passed them through
/// unparsed: Llama's `<function=name>{…}</function>`, Hermes and Qwen's
/// `<tool_call>{…}</tool_call>`, Mistral's `[TOOL_CALLS][…]` and Llama 3's
/// `<|python_tag|>{…}`. They count only when every one names an offered tool;
/// returns the text without them, and the calls.
///
/// A call inside a code block, a code span or a quoted line is something the
/// model is showing, not doing: a reply that quotes a page or a file which
/// holds such a call must not run it.
pub(crate) fn text_tool_calls(text: &str, offered: &[String]) -> Option<(String, Vec<ToolCall>)> {
    // Follows the reply's own text, leaving out the calls taken from it: a call's
    // arguments often hold fences and backticks of their own, as a file's content.
    let mut shown = Shown::default();
    const MARKERS: [&str; 4] = [
        "<function=",
        "<tool_call>",
        "[TOOL_CALLS]",
        "<|python_tag|>",
    ];
    fn json_at(s: &str) -> Option<(Value, usize)> {
        let lead = s.len() - s.trim_start().len();
        let mut values = serde_json::Deserializer::from_str(&s[lead..]).into_iter::<Value>();
        if let Some(Ok(value)) = values.next() {
            return Some((value, lead + values.byte_offset()));
        }
        // Models often put raw newlines and tabs inside strings, as in a file's content.
        let end = balanced(&s[lead..])?;
        let value = serde_json::from_str(&escape_controls(&s[lead..lead + end])).ok()?;
        Some((value, lead + end))
    }
    fn named(value: &Value) -> Option<(String, Value)> {
        let name = value["name"].as_str()?.to_owned();
        let arguments = value
            .get("arguments")
            .or_else(|| value.get("parameters"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        Some((name, arguments))
    }
    let mut found: Vec<(String, Value)> = vec![];
    let mut kept = String::new();
    let mut rest = text;
    while let Some((at, marker)) = MARKERS
        .iter()
        .filter_map(|m| rest.find(m).map(|i| (i, *m)))
        .min_by_key(|(i, _)| *i)
    {
        let after = &rest[at + marker.len()..];
        shown.read(&rest[..at]);
        if shown.here() {
            shown.read(marker);
            kept.push_str(&rest[..at + marker.len()]);
            rest = after;
            continue;
        }
        let parsed = match marker {
            "<function=" => after.find('>').and_then(|close| {
                let name = after[..close].trim().to_owned();
                json_at(&after[close + 1..])
                    .map(|(args, used)| (vec![(name, args)], close + 1 + used, Some("</function>")))
            }),
            "[TOOL_CALLS]" => json_at(after).map(|(value, used)| {
                let items = match value {
                    Value::Array(items) => items,
                    one => vec![one],
                };
                (items.iter().filter_map(named).collect(), used, None)
            }),
            _ => json_at(after).map(|(value, used)| {
                let closing = (marker == "<tool_call>").then_some("</tool_call>");
                (named(&value).into_iter().collect(), used, closing)
            }),
        };
        match parsed.filter(|(calls, _, _)| !calls.is_empty()) {
            Some((calls, used, closing)) => {
                kept.push_str(&rest[..at]);
                let mut tail = &after[used..];
                if let Some(end) = closing.and_then(|c| tail.trim_start().strip_prefix(c)) {
                    tail = end;
                }
                found.extend(calls);
                rest = tail;
            }
            None => {
                shown.read(marker);
                kept.push_str(&rest[..at + marker.len()]);
                rest = after;
            }
        }
    }
    kept.push_str(rest);
    if found.is_empty() {
        // Llama 4's pythonic form: [name(key="value", ...), ...]
        let (start, end, calls) = pythonic_calls(text, offered)?;
        let mut before = Shown::default();
        before.read(&text[..start]);
        if before.here() {
            return None;
        }
        kept = format!("{}{}", &text[..start], &text[end..]);
        found = calls;
    }
    if found.iter().any(|(name, _)| !offered.contains(name)) {
        return None;
    }
    let calls = found
        .into_iter()
        .enumerate()
        .map(|(i, (name, arguments))| ToolCall {
            id: format!("text_call_{i}"),
            name,
            arguments: match arguments {
                Value::String(raw) => raw,
                other => other.to_string(),
            },
        })
        .collect();
    Some((kept.trim().to_owned(), calls))
}

/// Whether a point in a reply is inside what Markdown shows as code or as a
/// quotation: a fenced block (to the end of the text when the fence is left
/// open), a line that starts with `>`, or a span between backticks on one line.
/// Fed the reply's text in order, it answers for the point reached.
#[derive(Default)]
struct Shown {
    fence: Option<&'static str>,
    /// How the current line starts, past its indent: enough to tell a fence or a quote.
    head: String,
    /// Backticks on the current line so far.
    ticks: usize,
}

impl Shown {
    const FENCES: [&'static str; 2] = ["```", "~~~"];

    fn read(&mut self, text: &str) {
        for c in text.chars() {
            match c {
                '\n' => {
                    let mark = Self::FENCES.into_iter().find(|m| self.head.starts_with(m));
                    match (self.fence, mark) {
                        (Some(open), Some(close)) if open == close => self.fence = None,
                        (None, Some(open)) => self.fence = Some(open),
                        _ => {}
                    }
                    self.head.clear();
                    self.ticks = 0;
                }
                _ => {
                    if c == '`' {
                        self.ticks += 1;
                    }
                    if self.head.len() < 3 && !(self.head.is_empty() && c.is_whitespace()) {
                        self.head.push(c);
                    }
                }
            }
        }
    }

    fn here(&self) -> bool {
        self.fence.is_some()
            || self.head.starts_with('>')
            || Self::FENCES.iter().any(|m| self.head.starts_with(m))
            || self.ticks % 2 == 1
    }
}

/// Where the JSON object or array at the start of `s` ends, counting brackets
/// outside strings; a string may hold raw control characters.
fn balanced(s: &str) -> Option<usize> {
    if !s.starts_with(['{', '[']) {
        return None;
    }
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for (i, c) in s.char_indices() {
        if quoted {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// JSON text with control characters inside strings escaped.
fn escape_controls(raw: &str) -> String {
    let (mut out, mut quoted, mut escaped) = (String::with_capacity(raw.len()), false, false);
    for c in raw.chars() {
        if quoted && !escaped && c.is_control() {
            match c {
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                other => out.push_str(&format!("\\u{:04x}", other as u32)),
            }
            continue;
        }
        if quoted {
            escaped = !escaped && c == '\\';
            if !escaped && c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
        }
        out.push(c);
    }
    out
}

/// A tool's name and its arguments, as parsed from text.
type NamedCall = (String, Value);

/// The first `[name(key=value, ...), ...]` list in `text` whose calls all name
/// offered tools, with its byte range. Values are Python literals: strings
/// (triple-quoted too), numbers, True, False, None, lists and dicts.
fn pythonic_calls(text: &str, offered: &[String]) -> Option<(usize, usize, Vec<NamedCall>)> {
    struct P<'a> {
        s: &'a str,
        i: usize,
    }
    impl P<'_> {
        fn rest(&self) -> &str {
            &self.s[self.i..]
        }
        fn space(&mut self) {
            self.i = self.s.len() - self.rest().trim_start().len();
        }
        fn eat(&mut self, token: &str) -> bool {
            self.space();
            let ok = self.rest().starts_with(token);
            if ok {
                self.i += token.len();
            }
            ok
        }
        fn ident(&mut self) -> Option<String> {
            self.space();
            let n = self
                .rest()
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(self.rest().len());
            if n == 0 || self.rest().starts_with(|c: char| c.is_ascii_digit()) {
                return None;
            }
            let name = self.rest()[..n].to_owned();
            self.i += n;
            Some(name)
        }
        fn string(&mut self) -> Option<String> {
            self.space();
            let quote = ["\"\"\"", "'''", "\"", "'"]
                .into_iter()
                .find(|q| self.rest().starts_with(q))?;
            self.i += quote.len();
            let body = self.rest();
            let mut out = String::new();
            let mut chars = body.char_indices();
            while let Some((at, c)) = chars.next() {
                if body[at..].starts_with(quote) {
                    self.i += at + quote.len();
                    return Some(out);
                }
                if c == '\\' {
                    match chars.next()?.1 {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '0' => out.push('\0'),
                        other => out.push(other),
                    }
                } else {
                    out.push(c);
                }
            }
            None
        }
        fn value(&mut self) -> Option<Value> {
            self.space();
            if self.rest().starts_with(['"', '\'']) {
                return self.string().map(Value::String);
            }
            for (word, value) in [
                ("True", json!(true)),
                ("False", json!(false)),
                ("None", Value::Null),
            ] {
                if self.rest().starts_with(word) {
                    self.i += word.len();
                    return Some(value);
                }
            }
            if self.eat("[") {
                let mut items = vec![];
                while !self.eat("]") {
                    items.push(self.value()?);
                    if !self.eat(",") && !self.rest().trim_start().starts_with(']') {
                        return None;
                    }
                }
                return Some(Value::Array(items));
            }
            if self.eat("{") {
                let mut map = serde_json::Map::new();
                while !self.eat("}") {
                    let key = self.string()?;
                    if !self.eat(":") {
                        return None;
                    }
                    map.insert(key, self.value()?);
                    if !self.eat(",") && !self.rest().trim_start().starts_with('}') {
                        return None;
                    }
                }
                return Some(Value::Object(map));
            }
            let n = self
                .rest()
                .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')))
                .unwrap_or(self.rest().len());
            let number = serde_json::from_str::<serde_json::Number>(&self.rest()[..n]).ok()?;
            self.i += n;
            Some(Value::Number(number))
        }
        fn call(&mut self) -> Option<(String, Value)> {
            let name = self.ident()?;
            if !self.eat("(") {
                return None;
            }
            let mut args = serde_json::Map::new();
            while !self.eat(")") {
                let key = self.ident()?;
                if !self.eat("=") {
                    return None;
                }
                args.insert(key, self.value()?);
                if !self.eat(",") && !self.rest().trim_start().starts_with(')') {
                    return None;
                }
            }
            Some((name, Value::Object(args)))
        }
    }
    for (start, _) in text.match_indices('[') {
        let after = text[start + 1..].trim_start();
        if !offered.iter().any(|name| {
            after.starts_with(name.as_str()) && after[name.len()..].trim_start().starts_with('(')
        }) {
            continue;
        }
        let mut p = P {
            s: text,
            i: start + 1,
        };
        let mut calls = vec![];
        let parsed = loop {
            match p.call() {
                Some(call) => calls.push(call),
                None => break false,
            }
            if p.eat("]") {
                break true;
            }
            if !p.eat(",") {
                break false;
            }
        };
        if parsed && calls.iter().all(|(name, _)| offered.contains(name)) {
            return Some((start, p.i, calls));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_two_calls_in_a_turn_share_an_id() {
        let call = |id: &str| ToolCall {
            id: id.into(),
            name: "t".into(),
            arguments: "{}".into(),
        };
        let mut used = std::collections::HashSet::new();
        let mut first = vec![call("text_call_0"), call("text_call_1")];
        unique_call_ids(&mut first, &mut used, 1);
        assert_eq!(first[0].id, "text_call_0");
        assert_eq!(first[1].id, "text_call_1");
        // The next step numbers its calls from zero again, and one repeats inside it.
        let mut second = vec![call("text_call_0"), call("abc"), call("abc")];
        unique_call_ids(&mut second, &mut used, 2);
        assert_eq!(second[0].id, "call_s2_0");
        assert_eq!(second[1].id, "abc");
        assert_eq!(second[2].id, "call_s2_2");
        // A model that happens to use the replacement's name still gets its own.
        let mut third = vec![call("call_s3_0"), call("call_s3_0")];
        unique_call_ids(&mut third, &mut used, 3);
        assert_eq!(third[0].id, "call_s3_0");
        assert_eq!(third[1].id, "call_s3_1");
        assert_eq!(used.len(), 7);
    }

    #[test]
    fn tool_calls_written_as_text_run_when_they_name_offered_tools() {
        let offered = vec!["delegate".to_string(), "write_file".to_string()];
        let (text, calls) = text_tool_calls(
            "Let me split this.\n<function=delegate>{\"tasks\": [{\"task\": \"a\"}]}</function>",
            &offered,
        )
        .unwrap();
        assert_eq!(text, "Let me split this.");
        assert_eq!(calls[0].name, "delegate");
        assert_eq!(calls[0].arguments, r#"{"tasks":[{"task":"a"}]}"#);
        let (text, calls) = text_tool_calls(
            "<tool_call>\n{\"name\": \"write_file\", \"arguments\": {\"path\": \"a.txt\"}}\n</tool_call>",
            &offered,
        )
        .unwrap();
        assert_eq!((text.as_str(), calls.len()), ("", 1));
        let (_, calls) = text_tool_calls(
            r#"[TOOL_CALLS][{"name":"write_file","arguments":{"path":"a"}},{"name":"delegate","arguments":"{}"}]"#,
            &offered,
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].arguments, "{}");
        // A call the model shows, in a code block, a code span or a quotation, is
        // not one it makes: quoting a page that holds a call must not run it.
        let call = r#"<function=write_file>{"path": "a.txt"}</function>"#;
        for quoted in [
            format!("The page says:\n```html\n{call}\n```\nI will not run that."),
            format!("The page says:\n~~~\n{call}\n"),
            format!("> {call}\n\nThat was in the file."),
            format!("It contains `{call}` in a comment."),
            "```\n[write_file(path=\"a\")]\n```".to_string(),
        ] {
            assert!(text_tool_calls(&quoted, &offered).is_none(), "{quoted}");
        }
        // What a call writes may hold fences of its own; the call after it still runs.
        let (_, calls) = text_tool_calls(
            "<function=write_file>{\"path\": \"README.md\", \"content\": \"Run:\n```bash\nnpm i\n\"}</function>\n<function=write_file>{\"path\": \"b.txt\"}</function>",
            &offered,
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        // One made after a code block still runs, and the block stays in the text.
        let (text, calls) =
            text_tool_calls(&format!("```\nx = 1\n```\nSaving it.\n{call}"), &offered).unwrap();
        assert_eq!(
            (text.as_str(), calls.len()),
            ("```\nx = 1\n```\nSaving it.", 1)
        );
        // An unknown tool, prose that mentions the syntax, or broken JSON stays text.
        assert!(text_tool_calls(r#"<function=rm_rf>{}</function>"#, &offered).is_none());
        assert!(text_tool_calls("Models write <function=name> tags.", &offered).is_none());
        assert!(text_tool_calls("<tool_call>{not json</tool_call>", &offered).is_none());
        // Raw newlines inside a JSON string, as models write file contents.
        let (_, calls) = text_tool_calls(
            "<function=write_file>{\"path\": \"a.py\", \"content\": \"import re\n\tx = 1\n\"}</function>",
            &offered,
        )
        .unwrap();
        assert_eq!(
            calls[0].arguments,
            r#"{"content":"import re\n\tx = 1\n","path":"a.py"}"#
        );
        // Llama 4's pythonic form, with a triple-quoted string and a nested list.
        let (text, calls) = text_tool_calls(
            "First the page.\n\n[write_file(path=\"index.html\", content='''<p class=\"x\">it's</p>\n'''), delegate(tasks=[{\"task\": \"a\", \"n\": 2}], wait=True)]",
            &offered,
        )
        .unwrap();
        assert_eq!(text, "First the page.");
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].arguments,
            r#"{"content":"<p class=\"x\">it's</p>\n","path":"index.html"}"#
        );
        assert_eq!(
            calls[1].arguments,
            r#"{"tasks":[{"n":2,"task":"a"}],"wait":true}"#
        );
        assert!(text_tool_calls("Use [write_file(path=x)] carefully", &offered).is_none());
        assert!(text_tool_calls("See [rm(path=\"/\")]", &offered).is_none());
    }

    #[test]
    fn a_file_being_typed_shows_its_last_lines() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "write_file".into(),
            arguments:
                r#"{"path":"js/rig.js","content":"const A = 1;\nconst \"B\" = 2;\n// θé and mo"#
                    .into(),
        };
        let draft = draft_of(&call).unwrap();
        assert_eq!(draft["path"], "js/rig.js");
        assert_eq!(draft["lines"], 3);
        assert_eq!(
            draft["tail"],
            "const A = 1;\nconst \"B\" = 2;\n// θé and mo"
        );
        let reading = ToolCall {
            name: "read_file".into(),
            ..call
        };
        assert!(draft_of(&reading).is_none());
        assert_eq!(
            partial_string(r#"{"path": "a b"}"#, "path").as_deref(),
            Some("a b")
        );
        assert_eq!(partial_string(r#"{"content":"x"}"#, "path"), None);
    }
}
