//! What an app is and how it runs. An app is a list of steps over typed inputs; Decide
//! and If hold paths of their own steps, of which exactly one runs, and For each runs
//! its steps once per item. A step reads the run through `{{...}}` references: a
//! string that is only a reference takes the value itself, anything else gets it as
//! text. Every step runs inside Chat: models through the gateway as the run's person,
//! knowledge through its search, HTTP through the public-only client, and code in
//! QuickJS (code.rs).
use super::code;
use crate::{auth::Identity, AppState};
use futures_util::{future::BoxFuture, stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub const MAX_STEPS: usize = 60;
const MAX_DEPTH: usize = 4;
const MAX_INPUTS: usize = 20;
const MAX_PATHS: usize = 10;
const MAX_ITEMS: usize = 100;
const MODEL_TIMEOUT: Duration = Duration::from_secs(180);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_BYTES: usize = 1024 * 1024;
/// How much of each step's output the trace keeps.
const PREVIEW_BYTES: usize = 2000;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    #[serde(default)]
    pub inputs: Vec<Field>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: Kind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Text,
    Number,
    Boolean,
    Json,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Agent {
        id: String,
        #[serde(default)]
        title: String,
        model: String,
        #[serde(default)]
        instructions: String,
        prompt: String,
        /// Read the reply as JSON.
        #[serde(default)]
        json: bool,
    },
    Decide {
        id: String,
        #[serde(default)]
        title: String,
        model: String,
        /// What the model is asked, such as "What is the question about?"
        #[serde(default)]
        question: String,
        /// What it judges, such as "{{inputs.question}}".
        input: String,
        paths: Vec<Choice>,
        #[serde(default)]
        otherwise: Vec<Step>,
    },
    If {
        id: String,
        #[serde(default)]
        title: String,
        paths: Vec<Branch>,
        #[serde(default)]
        otherwise: Vec<Step>,
    },
    ForEach {
        id: String,
        #[serde(default)]
        title: String,
        items: String,
        steps: Vec<Step>,
        #[serde(default)]
        concurrency: Option<u8>,
    },
    Search {
        id: String,
        #[serde(default)]
        title: String,
        /// None searches the project's collections.
        #[serde(default)]
        collections: Vec<Uuid>,
        query: String,
        #[serde(default)]
        limit: Option<u8>,
    },
    Http {
        id: String,
        #[serde(default)]
        title: String,
        #[serde(default = "get")]
        method: String,
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        body: Option<Value>,
    },
    Code {
        id: String,
        #[serde(default)]
        title: String,
        code: String,
    },
    Response {
        id: String,
        #[serde(default)]
        title: String,
        value: Value,
    },
}
fn get() -> String {
    "GET".into()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    /// Every rule must hold.
    pub when: Vec<Rule>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// A rule is a reference, a test and, for the tests that compare, what to compare
/// with; or one JavaScript expression.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub test: Option<Test>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Test {
    IsEmpty,
    NotEmpty,
    Equals,
    NotEquals,
    Contains,
    MoreThan,
    LessThan,
}

impl Step {
    pub fn id(&self) -> &str {
        match self {
            Step::Agent { id, .. }
            | Step::Decide { id, .. }
            | Step::If { id, .. }
            | Step::ForEach { id, .. }
            | Step::Search { id, .. }
            | Step::Http { id, .. }
            | Step::Code { id, .. }
            | Step::Response { id, .. } => id,
        }
    }
    fn title(&self) -> &str {
        match self {
            Step::Agent { title, .. }
            | Step::Decide { title, .. }
            | Step::If { title, .. }
            | Step::ForEach { title, .. }
            | Step::Search { title, .. }
            | Step::Http { title, .. }
            | Step::Code { title, .. }
            | Step::Response { title, .. } => title,
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Step::Agent { .. } => "agent",
            Step::Decide { .. } => "decide",
            Step::If { .. } => "if",
            Step::ForEach { .. } => "for_each",
            Step::Search { .. } => "search",
            Step::Http { .. } => "http",
            Step::Code { .. } => "code",
            Step::Response { .. } => "response",
        }
    }
    /// The step lists this step holds.
    fn children(&self) -> Vec<&[Step]> {
        match self {
            Step::Decide {
                paths, otherwise, ..
            } => paths
                .iter()
                .map(|p| p.steps.as_slice())
                .chain([otherwise.as_slice()])
                .collect(),
            Step::If {
                paths, otherwise, ..
            } => paths
                .iter()
                .map(|p| p.steps.as_slice())
                .chain([otherwise.as_slice()])
                .collect(),
            Step::ForEach { steps, .. } => vec![steps.as_slice()],
            _ => vec![],
        }
    }
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl Definition {
    /// Checks the shape a run relies on, so a saved app can always start.
    pub fn validate(&self) -> Result<(), String> {
        if self.inputs.len() > MAX_INPUTS {
            return Err(format!("An app takes at most {MAX_INPUTS} inputs"));
        }
        let mut names = HashSet::new();
        for f in &self.inputs {
            if !valid_name(&f.name) {
                return Err(format!(
                    "Input “{}”: use lowercase letters, digits and _, starting with a letter",
                    f.name
                ));
            }
            if !names.insert(f.name.as_str()) {
                return Err(format!("Two inputs are called “{}”", f.name));
            }
        }
        let mut ids = HashSet::new();
        let mut count = 0;
        check(&self.steps, 0, &mut ids, &mut count)
    }
}
fn check<'a>(
    steps: &'a [Step],
    depth: usize,
    ids: &mut HashSet<&'a str>,
    count: &mut usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("Paths can be nested at most {MAX_DEPTH} deep"));
    }
    for step in steps {
        *count += 1;
        if *count > MAX_STEPS {
            return Err(format!("An app has at most {MAX_STEPS} steps"));
        }
        let id = step.id();
        if !valid_name(id) {
            return Err(format!(
                "Step “{id}”: its id uses lowercase letters, digits and _, starting with a letter"
            ));
        }
        if !ids.insert(id) {
            return Err(format!("Two steps are called “{id}”"));
        }
        let at = |message: &str| format!("Step “{id}”: {message}");
        match step {
            Step::Agent { model, prompt, .. } => {
                if model.trim().is_empty() || prompt.trim().is_empty() {
                    return Err(at("choose a model and write the prompt"));
                }
            }
            Step::Decide {
                model,
                input,
                paths,
                ..
            } => {
                if model.trim().is_empty() || input.trim().is_empty() {
                    return Err(at("choose a model and what it judges"));
                }
                if paths.is_empty() || paths.len() > MAX_PATHS {
                    return Err(at(&format!("give it 1 to {MAX_PATHS} paths")));
                }
                if paths.iter().any(|p| p.name.trim().is_empty()) {
                    return Err(at("every path needs a name"));
                }
            }
            Step::If { paths, .. } => {
                if paths.is_empty() || paths.len() > MAX_PATHS {
                    return Err(at(&format!("give it 1 to {MAX_PATHS} paths")));
                }
                for rule in paths.iter().flat_map(|p| &p.when) {
                    let plain = rule.value.is_some() && rule.test.is_some();
                    if plain == rule.code.is_some() {
                        return Err(at("each rule is a value and a test, or one line of code"));
                    }
                    if matches!(
                        rule.test,
                        Some(
                            Test::Equals
                                | Test::NotEquals
                                | Test::Contains
                                | Test::MoreThan
                                | Test::LessThan
                        )
                    ) && rule.to.is_none()
                    {
                        return Err(at("say what the rule compares with"));
                    }
                }
                if paths.iter().any(|p| p.when.is_empty()) {
                    return Err(at("every path needs at least one rule"));
                }
            }
            Step::ForEach {
                items, concurrency, ..
            } => {
                if items.trim().is_empty() {
                    return Err(at("say which list it goes through"));
                }
                if concurrency.is_some_and(|c| !(1..=8).contains(&c)) {
                    return Err(at("run 1 to 8 items at once"));
                }
            }
            Step::Search { query, limit, .. } => {
                if query.trim().is_empty() {
                    return Err(at("write what it searches for"));
                }
                if limit.is_some_and(|l| !(1..=20).contains(&l)) {
                    return Err(at("find 1 to 20 passages"));
                }
            }
            Step::Http { method, url, .. } => {
                if !["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&method.as_str()) {
                    return Err(at("use GET, POST, PUT, PATCH or DELETE"));
                }
                if url.trim().is_empty() {
                    return Err(at("give it a URL"));
                }
            }
            Step::Code { code, .. } => {
                if code.trim().is_empty() || code.len() > 20_000 {
                    return Err(at("write up to 20,000 characters of code"));
                }
            }
            Step::Response { .. } => {}
        }
        for list in step.children() {
            check(list, depth + 1, ids, count)?;
        }
    }
    Ok(())
}

/// The inputs a run was given, checked against the app's fields.
pub fn inputs(definition: &Definition, given: &Value) -> Result<Value, String> {
    let given = match given {
        Value::Null => Map::new(),
        Value::Object(m) => m.clone(),
        _ => return Err("Send the inputs as a JSON object".into()),
    };
    if let Some(extra) = given
        .keys()
        .find(|k| !definition.inputs.iter().any(|f| &f.name == *k))
    {
        return Err(format!("This app has no input called “{extra}”"));
    }
    let mut out = Map::new();
    for f in &definition.inputs {
        match given.get(&f.name) {
            None | Some(Value::Null) if f.required => {
                return Err(format!("The input “{}” is required", f.name))
            }
            None | Some(Value::Null) => {
                out.insert(f.name.clone(), Value::Null);
            }
            Some(v) => {
                let ok = match f.kind {
                    Kind::Text => v.is_string(),
                    Kind::Number => v.is_number(),
                    Kind::Boolean => v.is_boolean(),
                    Kind::Json => true,
                };
                if !ok {
                    return Err(format!(
                        "The input “{}” must be {}",
                        f.name,
                        match f.kind {
                            Kind::Text => "text",
                            Kind::Number => "a number",
                            Kind::Boolean => "true or false",
                            Kind::Json => "JSON",
                        }
                    ));
                }
                out.insert(f.name.clone(), v.clone());
            }
        }
    }
    Ok(Value::Object(out))
}

// ---- References ----

/// The value at a dotted path such as `steps.search.0.content`, or null.
pub fn lookup(view: &Value, path: &str) -> Value {
    let mut at = view;
    for part in path.trim().split('.') {
        at = match at {
            Value::Object(m) => match m.get(part) {
                Some(v) => v,
                None => return Value::Null,
            },
            Value::Array(a) => match part.parse::<usize>().ok().and_then(|i| a.get(i)) {
                Some(v) => v,
                None => return Value::Null,
            },
            _ => return Value::Null,
        };
    }
    at.clone()
}
fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
/// A string with its references filled in: a lone reference keeps its value.
pub fn fill(text: &str, view: &Value) -> Value {
    let trimmed = text.trim();
    if let Some(inner) = trimmed
        .strip_prefix("{{")
        .and_then(|s| s.strip_suffix("}}"))
        .filter(|s| !s.contains("{{") && !s.contains("}}"))
    {
        return lookup(view, inner);
    }
    Value::String(fill_text(text, view))
}
pub fn fill_text(text: &str, view: &Value) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("}}") {
            Some(end) => {
                out.push_str(&as_text(&lookup(view, &rest[start + 2..start + 2 + end])));
                rest = &rest[start + 2 + end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}
/// Any JSON with its strings filled in.
pub fn fill_value(value: &Value, view: &Value) -> Value {
    match value {
        Value::String(s) => fill(s, view),
        Value::Array(a) => Value::Array(a.iter().map(|v| fill_value(v, view)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), fill_value(v, view)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
        _ => false,
    }
}
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}
/// Whether a plain rule holds.
pub fn test(value: &Value, test: Test, to: &str) -> bool {
    match test {
        Test::IsEmpty => empty(value),
        Test::NotEmpty => !empty(value),
        Test::Equals => as_text(value) == to,
        Test::NotEquals => as_text(value) != to,
        Test::Contains => match value {
            Value::Array(a) => a.iter().any(|v| as_text(v) == to),
            other => as_text(other).to_lowercase().contains(&to.to_lowercase()),
        },
        Test::MoreThan | Test::LessThan => {
            let len = |v: &Value| match v {
                Value::Array(a) => Some(a.len() as f64),
                other => number(other),
            };
            match (len(value), to.trim().parse::<f64>().ok()) {
                (Some(a), Some(b)) if test == Test::MoreThan => a > b,
                (Some(a), Some(b)) => a < b,
                _ => false,
            }
        }
    }
}

// ---- Running ----

/// What the steps so far have made, as a step sees it.
#[derive(Clone)]
struct Scope {
    inputs: Value,
    steps: Map<String, Value>,
    item: Value,
    index: Value,
}
impl Scope {
    fn view(&self) -> Value {
        json!({"inputs": self.inputs, "steps": self.steps, "item": self.item, "index": self.index})
    }
}
enum Flow {
    Next(Value),
    Respond(Value),
}

pub struct Run<'a> {
    pub state: &'a AppState,
    /// Whose models, knowledge and allowance the run uses.
    pub user: Identity,
    /// The collections a Search step with none of its own searches.
    pub collections: Vec<Uuid>,
    pub trace: Mutex<Vec<Value>>,
}

impl Run<'_> {
    /// Runs the app: what its Response returned, or, with none, the last step's output.
    pub async fn go(&self, definition: &Definition, inputs: Value) -> Result<Value, String> {
        let mut scope = Scope {
            inputs,
            steps: Map::new(),
            item: Value::Null,
            index: Value::Null,
        };
        match self.steps(&definition.steps, &mut scope, false).await? {
            Flow::Next(v) | Flow::Respond(v) => Ok(v),
        }
    }

    /// One list of steps; inside For each, a Response is the item's result.
    fn steps<'s>(
        &'s self,
        list: &'s [Step],
        scope: &'s mut Scope,
        in_item: bool,
    ) -> BoxFuture<'s, Result<Flow, String>> {
        Box::pin(async move {
            let mut last = Value::Null;
            for step in list {
                let started = Instant::now();
                let outcome = self.step(step, scope, in_item).await;
                let ms = started.elapsed().as_millis() as u64;
                let (flow, path) = match outcome {
                    Ok(done) => done,
                    Err(e) => {
                        self.note(step, ms, Err(&e), None);
                        let name = if step.title().is_empty() {
                            step.id()
                        } else {
                            step.title()
                        };
                        return Err(format!("{name}: {e}"));
                    }
                };
                let value = match &flow {
                    Flow::Next(v) | Flow::Respond(v) => v.clone(),
                };
                self.note(step, ms, Ok(&value), path);
                scope.steps.insert(step.id().to_owned(), value.clone());
                match flow {
                    Flow::Respond(v) if !in_item => return Ok(Flow::Respond(v)),
                    Flow::Respond(v) => return Ok(Flow::Next(v)),
                    Flow::Next(v) => last = v,
                }
            }
            Ok(Flow::Next(last))
        })
    }

    fn note(&self, step: &Step, ms: u64, result: Result<&Value, &String>, path: Option<String>) {
        let mut entry =
            json!({"id": step.id(), "type": step.kind(), "title": step.title(), "ms": ms});
        match result {
            Ok(v) => {
                let text = v.to_string();
                entry["ok"] = json!(true);
                entry["output"] = json!(if text.len() > PREVIEW_BYTES {
                    let cut = (0..=PREVIEW_BYTES)
                        .rev()
                        .find(|i| text.is_char_boundary(*i))
                        .unwrap_or(0);
                    format!("{}…", &text[..cut])
                } else {
                    text
                });
            }
            Err(e) => {
                entry["ok"] = json!(false);
                entry["error"] = json!(e);
            }
        }
        if let Some(path) = path {
            entry["path"] = json!(path);
        }
        if let Ok(mut trace) = self.trace.lock() {
            trace.push(entry);
        }
    }

    /// One step: its flow, and for a branch the path that ran.
    async fn step(
        &self,
        step: &Step,
        scope: &mut Scope,
        in_item: bool,
    ) -> Result<(Flow, Option<String>), String> {
        let view = scope.view();
        let next = |v: Value| Ok((Flow::Next(v), None));
        match step {
            Step::Agent {
                model,
                instructions,
                prompt,
                json,
                ..
            } => {
                let mut messages = vec![];
                let system = fill_text(instructions, &view);
                if !system.trim().is_empty() {
                    messages.push(json!({"role": "system", "content": system}));
                }
                messages.push(json!({"role": "user", "content": fill_text(prompt, &view)}));
                let gateway = self
                    .state
                    .gateway
                    .as_ref()
                    .ok_or("No gateway is connected")?;
                let reply = gateway
                    .complete(
                        &self.user,
                        &json!({"model": model, "messages": messages, "stream": false}),
                        MODEL_TIMEOUT,
                    )
                    .await
                    .map_err(|e| e.1)?;
                let text = reply["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if !*json {
                    return next(Value::String(text));
                }
                let bare = text
                    .trim_start_matches("```json")
                    .trim_start_matches("```")
                    .trim_end_matches("```")
                    .trim();
                next(
                    serde_json::from_str(bare)
                        .map_err(|_| "The model's reply wasn't JSON".to_string())?,
                )
            }
            Step::Decide {
                model,
                question,
                input,
                paths,
                otherwise,
                ..
            } => {
                let mut criteria = Map::new();
                for (i, p) in paths.iter().enumerate() {
                    let text = if p.description.trim().is_empty() {
                        p.name.clone()
                    } else {
                        format!("{}: {}", p.name, p.description)
                    };
                    criteria.insert(format!("p{i:02}"), json!(text));
                }
                criteria.insert("other".into(), json!("None of these"));
                let ask = if question.trim().is_empty() {
                    "Which of these fits best?".to_owned()
                } else {
                    fill_text(question, &view)
                };
                let body = json!({
                    "state": {"input": fill_text(input, &view)},
                    "questions": {"path": {"type": "choice", "instructions": ask, "criteria": criteria}},
                    "model": model,
                    "on_no_answer": "low_confidence",
                });
                let gateway = self
                    .state
                    .gateway
                    .as_ref()
                    .ok_or("No gateway is connected")?;
                let answer = gateway
                    .verdicts(&self.user, &body, MODEL_TIMEOUT)
                    .await
                    .map_err(|e| e.1)?;
                let picked = answer["verdicts"]["path"]["value"]
                    .as_str()
                    .and_then(|v| v.strip_prefix('p'))
                    .and_then(|i| i.parse::<usize>().ok())
                    .and_then(|i| paths.get(i));
                let (name, list) = match picked {
                    Some(p) => (p.name.clone(), &p.steps),
                    None => ("Otherwise".to_owned(), otherwise),
                };
                let flow = self.steps(list, scope, in_item).await?;
                Ok((flow, Some(name)))
            }
            Step::If {
                paths, otherwise, ..
            } => {
                let mut chosen = None;
                'paths: for (i, p) in paths.iter().enumerate() {
                    for rule in &p.when {
                        let holds = match (&rule.code, &rule.value, rule.test) {
                            (Some(code), _, _) => code::holds(code.clone(), view.clone()).await?,
                            (None, Some(value), Some(t)) => test(
                                &fill(value, &view),
                                t,
                                &fill_text(rule.to.as_deref().unwrap_or_default(), &view),
                            ),
                            _ => false,
                        };
                        if !holds {
                            continue 'paths;
                        }
                    }
                    chosen = Some(i);
                    break;
                }
                let (name, list) = match chosen {
                    Some(i) => (format!("Path {}", i + 1), &paths[i].steps),
                    None => ("Otherwise".to_owned(), otherwise),
                };
                let flow = self.steps(list, scope, in_item).await?;
                Ok((flow, Some(name)))
            }
            Step::ForEach {
                items,
                steps,
                concurrency,
                ..
            } => {
                let list = match fill(items, &view) {
                    Value::Array(a) => a,
                    Value::Null => vec![],
                    _ => return Err("its items aren't a list".into()),
                };
                if list.len() > MAX_ITEMS {
                    return Err(format!(
                        "it got {} items; the most is {MAX_ITEMS}",
                        list.len()
                    ));
                }
                let base = scope.clone();
                let results: Vec<Result<Value, String>> =
                    stream::iter(list.into_iter().enumerate())
                        .map(|(i, item)| {
                            let mut each = base.clone();
                            each.item = item;
                            each.index = json!(i);
                            async move {
                                match self.steps(steps, &mut each, true).await? {
                                    Flow::Next(v) | Flow::Respond(v) => Ok(v),
                                }
                            }
                        })
                        .buffered(concurrency.unwrap_or(4) as usize)
                        .collect()
                        .await;
                next(Value::Array(results.into_iter().collect::<Result<_, _>>()?))
            }
            Step::Search {
                collections,
                query,
                limit,
                ..
            } => {
                let knowledge = self
                    .state
                    .knowledge
                    .as_ref()
                    .ok_or("Knowledge isn't set up here")?;
                let gateway = self
                    .state
                    .gateway
                    .as_ref()
                    .ok_or("No gateway is connected")?;
                let from = if collections.is_empty() {
                    &self.collections
                } else {
                    collections
                };
                let hits = knowledge
                    .search(
                        gateway,
                        &self.user,
                        from,
                        &fill_text(query, &view),
                        limit.unwrap_or(8) as usize,
                    )
                    .await
                    .map_err(|e| e.1)?;
                next(Value::Array(
                    hits.iter()
                        .map(|h| json!({"name": h.name, "page": h.page, "section": h.section, "content": h.content}))
                        .collect(),
                ))
            }
            Step::Http {
                method,
                url,
                headers,
                body,
                ..
            } => {
                let url = crate::connections::mcp::check_url(&fill_text(url, &view), false)?;
                let client = crate::connections::mcp::client(false, None)
                    .map_err(|_| "Couldn't make the request".to_string())?;
                let method = reqwest::Method::from_bytes(method.as_bytes())
                    .map_err(|_| "Unknown method".to_string())?;
                let mut request = client.request(method, url).timeout(HTTP_TIMEOUT);
                for (k, v) in headers {
                    request = request.header(k.as_str(), fill_text(v, &view));
                }
                request = match body.as_ref().map(|b| fill_value(b, &view)) {
                    None | Some(Value::Null) => request,
                    Some(Value::String(s)) => request.body(s),
                    Some(other) => request.json(&other),
                };
                let mut response = request.send().await.map_err(|e| {
                    if e.is_timeout() {
                        "The server didn't answer within 30 seconds".to_string()
                    } else {
                        "Couldn't reach the server".to_string()
                    }
                })?;
                let status = response.status().as_u16();
                let is_json = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.contains("json"));
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| "The answer broke off".to_string())?
                {
                    if bytes.len() + chunk.len() > HTTP_BYTES {
                        return Err("The answer was bigger than 1 MB".into());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let body = if is_json {
                    serde_json::from_str(&text).unwrap_or(Value::String(text))
                } else {
                    Value::String(text)
                };
                next(json!({"status": status, "body": body}))
            }
            Step::Code { code, .. } => next(code::run(code.clone(), view).await?),
            Step::Response { value, .. } => Ok((Flow::Respond(fill_value(value, &view)), None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_take_the_value_alone_and_text_inside_a_sentence() {
        let view = json!({"inputs": {"q": "IC50?"}, "steps": {"rows": [{"id": "S-1"}], "n": 3}});
        assert_eq!(fill("{{steps.rows}}", &view), json!([{"id": "S-1"}]));
        assert_eq!(fill(" {{ steps.n }} ", &view), json!(3));
        assert_eq!(
            fill(
                "Q: {{inputs.q}} ({{steps.n}} rows, first {{steps.rows.0.id}})",
                &view
            ),
            json!("Q: IC50? (3 rows, first S-1)")
        );
        assert_eq!(fill("{{steps.missing}}", &view), Value::Null);
        assert_eq!(fill("{{ unclosed", &view), json!("{{ unclosed"));
        assert_eq!(
            fill_value(
                &json!({"answer": "{{inputs.q}}", "rows": "{{steps.rows}}"}),
                &view
            ),
            json!({"answer": "IC50?", "rows": [{"id": "S-1"}]})
        );
    }

    #[test]
    fn rules_test_values_the_way_people_mean_them() {
        assert!(test(&json!([]), Test::IsEmpty, ""));
        assert!(test(&json!("  "), Test::IsEmpty, ""));
        assert!(test(&json!("Please, URGENT"), Test::Contains, "urgent"));
        assert!(test(&json!(["a", "b"]), Test::Contains, "b"));
        assert!(test(&json!([1, 2, 3]), Test::MoreThan, "2"));
        assert!(test(&json!("10"), Test::LessThan, "11"));
        assert!(test(&json!(5), Test::Equals, "5"));
        assert!(!test(&json!("x"), Test::MoreThan, "1"));
    }

    #[test]
    fn a_definition_is_checked_before_it_is_saved_and_inputs_before_a_run() {
        let good: Definition = serde_json::from_value(json!({
            "inputs": [{"name": "question", "required": true}, {"name": "limit", "type": "number"}],
            "steps": [
                {"type": "if", "id": "check", "paths": [{"when": [{"value": "{{inputs.question}}", "test": "is_empty"}], "steps": [{"type": "response", "id": "nothing", "value": "Ask something"}]}]},
                {"type": "code", "id": "shape", "code": "return 1;"}
            ]
        }))
        .unwrap();
        good.validate().unwrap();
        assert_eq!(
            inputs(&good, &json!({"question": "Hi"})).unwrap(),
            json!({"question": "Hi", "limit": null})
        );
        assert!(inputs(&good, &json!({})).unwrap_err().contains("required"));
        assert!(inputs(&good, &json!({"question": "Hi", "limit": "3"}))
            .unwrap_err()
            .contains("number"));
        assert!(inputs(&good, &json!({"question": "Hi", "other": 1}))
            .unwrap_err()
            .contains("no input"));
        let bad = |steps: Value| {
            serde_json::from_value::<Definition>(json!({"steps": steps}))
                .map_err(|e| e.to_string())
                .and_then(|d| d.validate())
                .unwrap_err()
        };
        assert!(bad(json!([{"type": "code", "id": "a", "code": "1"}, {"type": "code", "id": "a", "code": "2"}])).contains("Two steps"));
        assert!(bad(json!([{"type": "code", "id": "A", "code": "1"}])).contains("lowercase"));
        assert!(
            bad(json!([{"type": "code", "id": "a", "code": "1", "extra": true}]))
                .contains("unknown field")
        );
        assert!(bad(json!([{"type": "shell", "id": "a"}])).contains("unknown variant"));
        assert!(bad(json!([{"type": "if", "id": "a", "paths": [{"when": [{"value": "{{x}}", "test": "equals"}]}]}])).contains("compares with"));
        let deep = json!([{"type": "for_each", "id": "a", "items": "{{x}}", "steps": [{"type": "for_each", "id": "b", "items": "{{x}}", "steps": [{"type": "for_each", "id": "c", "items": "{{x}}", "steps": [{"type": "for_each", "id": "d", "items": "{{x}}", "steps": [{"type": "for_each", "id": "e", "items": "{{x}}", "steps": [{"type": "code", "id": "f", "code": "1"}]}]}]}]}]}]);
        assert!(bad(deep).contains("nested"));
    }
}
