//! Decisions: short yes/no and pick-one questions Chat asks a model, such as whether a
//! research run really tested its idea. They use the verdicts format (`POST
//! /v1/verdicts`: a state, typed questions, and a probability for every answer), so the
//! gateway or any other service that speaks it can answer. Administrators choose the
//! endpoint in Settings: the gateway, as the person a question is for; another
//! endpoint, with its own key; or none. Nothing waits on an answer: when decisions are
//! off or fail, the work that asked goes on without one.

use std::{
    fmt,
    time::{Duration, Instant},
};

use axum::{
    extract::State,
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use reqwest::{redirect, Client, Response};
use serde_json::{json, Value};
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

use crate::{
    auth::Identity,
    error::ApiError,
    gateway::{valid_model, Gateway},
    secrets::SecretKey,
    AppState,
};

/// Where an endpoint given as an origin, or as an API base ending in /v1, takes questions.
const PATH: &str = "/v1/verdicts";
const MAX_URL: usize = 2048;
const MAX_KEY: usize = 4096;
const MAX_RESPONSE: usize = 1024 * 1024;
/// The custom endpoint's key is sealed bound to this, in format 1.
const KEY_AAD: &[u8] = b"obleth-chat decisions endpoint key";
const KEY_FORMAT: u8 = 1;
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// Where questions go.
#[derive(Clone, PartialEq)]
pub enum Endpoint {
    /// The gateway's /v1/verdicts, as the person a question is for.
    Gateway,
    /// Another service, with the administrator's key when one is saved.
    Custom {
        url: Url,
        key: Key,
    },
    Off,
}

#[derive(Clone, PartialEq)]
pub enum Key {
    None,
    Set(String),
    /// Saved, but sealed with a secret key this installation no longer has.
    Unreadable,
}

/// Never shows the key, so settings can reach a log.
impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gateway => f.write_str("Gateway"),
            Self::Off => f.write_str("Off"),
            Self::Custom { url, key } => f
                .debug_struct("Custom")
                .field("url", &url.as_str())
                .field(
                    "key",
                    &match key {
                        Key::None => "none",
                        Key::Set(_) => "set",
                        Key::Unreadable => "unreadable",
                    },
                )
                .finish(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub endpoint: Endpoint,
    /// None leaves the choice to the endpoint.
    pub model: Option<String>,
    /// Ask whether each research run tested its idea.
    pub check_runs: bool,
    /// Ask whether a new experiment belongs under the parent it was given.
    pub check_tree: bool,
    /// Ask whether a research report's statements are supported by the runs.
    pub check_reports: bool,
}

impl Settings {
    pub fn on(&self) -> bool {
        self.endpoint != Endpoint::Off
    }
}

/// A yes/no verdict.
#[derive(Clone, Debug, PartialEq)]
pub struct YesNo {
    /// The endpoint's name for the model that answered.
    pub model: String,
    /// The probability of yes.
    pub yes: f64,
    pub confidence: f64,
}

impl YesNo {
    /// A no the answer is sure of. A check that refuses or corrects something acts only on
    /// this: an unsure no about a true statement would teach the model the wrong thing.
    pub fn sure_no(&self) -> bool {
        self.yes < 0.5 && self.confidence >= 0.5
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    endpoint: String,
    url: Option<String>,
    api_key: Option<Vec<u8>>,
    model: Option<String>,
    check_runs: bool,
    check_tree: bool,
    check_reports: bool,
}

#[derive(Clone)]
pub struct Decisions {
    pool: PgPool,
    /// Seals the custom endpoint's key; without it no key can be saved.
    secret: Option<SecretKey>,
    client: Client,
}

impl Decisions {
    pub fn new(pool: PgPool, secret: Option<SecretKey>) -> Self {
        // A redirect would carry the key somewhere the administrator didn't choose.
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            pool,
            secret,
            client,
        }
    }

    async fn row(&self) -> Result<Row, sqlx::Error> {
        sqlx::query_as(
            "SELECT endpoint, url, api_key, model, check_runs, check_tree, check_reports FROM decisions WHERE id",
        )
            .fetch_one(&self.pool)
            .await
    }

    /// The settings in use now. Read on each use, so every replica follows a change.
    pub async fn settings(&self) -> Result<Settings, sqlx::Error> {
        Ok(self.resolve(&self.row().await?, None))
    }

    /// `key` is a new key not yet saved, which replaces the saved one.
    fn resolve(&self, row: &Row, key: Option<&str>) -> Settings {
        let url = row.url.as_deref().and_then(|u| Url::parse(u).ok());
        let endpoint = match (row.endpoint.as_str(), url) {
            ("gateway", _) => Endpoint::Gateway,
            ("custom", Some(url)) => Endpoint::Custom {
                url,
                key: match (key, &row.api_key) {
                    (Some(key), _) => Key::Set(key.to_owned()),
                    (None, Some(sealed)) => self.open(sealed).map_or(Key::Unreadable, Key::Set),
                    (None, None) => Key::None,
                },
            },
            _ => Endpoint::Off,
        };
        Settings {
            endpoint,
            model: row.model.clone(),
            check_runs: row.check_runs,
            check_tree: row.check_tree,
            check_reports: row.check_reports,
        }
    }

    fn open(&self, sealed: &[u8]) -> Option<String> {
        let plain = self.secret.as_ref()?.open(KEY_FORMAT, KEY_AAD, sealed)?;
        String::from_utf8(plain).ok()
    }

    /// Sends one verdicts request. `questions` maps ids to typed questions; a question
    /// the model can't answer comes back unanswered rather than failing the others.
    /// The error says why there is no answer, in words a person can read.
    pub async fn ask(
        &self,
        settings: &Settings,
        gateway: Option<&Gateway>,
        user: &Identity,
        state: Value,
        questions: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let mut body =
            json!({"state": state, "questions": questions, "on_no_answer": "low_confidence"});
        if let Some(model) = &settings.model {
            body["model"] = json!(model);
        }
        match &settings.endpoint {
            Endpoint::Off => Err("Decisions are off".into()),
            Endpoint::Gateway => {
                let gateway = gateway.ok_or("Chat has no gateway to ask")?;
                gateway
                    .verdicts(user, &body, timeout)
                    .await
                    .map_err(|e| e.1)
            }
            Endpoint::Custom { url, key } => self.post(url, key, &body, timeout).await,
        }
    }

    async fn post(
        &self,
        url: &Url,
        key: &Key,
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let mut request = self.client.post(url.clone()).timeout(timeout).json(body);
        match key {
            Key::Set(key) => request = request.bearer_auth(key),
            Key::Unreadable => return Err("The endpoint's saved key can't be read because the installation's secret key changed. Enter the key again.".into()),
            Key::None => {}
        }
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                "The endpoint did not answer in time".to_string()
            } else {
                "The endpoint could not be reached".to_string()
            }
        })?;
        let status = response.status();
        let bytes = read_capped(response)
            .await
            .ok_or("The endpoint's answer could not be read")?;
        if status.is_redirection() {
            return Err(format!(
                "The endpoint answered {} (a redirect); enter the address it redirects to",
                status.as_u16()
            ));
        }
        if !status.is_success() {
            return Err(match crate::gateway::reason(&bytes) {
                Some(reason) => format!("The endpoint answered {}: {reason}", status.as_u16()),
                None => format!("The endpoint answered {}", status.as_u16()),
            });
        }
        serde_json::from_slice(&bytes).map_err(|_| "The endpoint's answer is not JSON".into())
    }
}

/// What a turn's tools need to ask: where the settings live, the gateway, and the
/// person the questions are for.
#[derive(Clone, Copy)]
pub struct Asker<'a> {
    pub decisions: &'a Decisions,
    pub gateway: Option<&'a Gateway>,
    pub user: &'a Identity,
}

impl Asker<'_> {
    pub async fn settings(&self) -> Option<Settings> {
        self.decisions.settings().await.ok()
    }

    pub async fn ask(
        &self,
        settings: &Settings,
        state: Value,
        questions: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.decisions
            .ask(settings, self.gateway, self.user, state, questions, timeout)
            .await
    }
}

async fn read_capped(response: Response) -> Option<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(part) = stream.next().await {
        let part = part.ok()?;
        if bytes.len() + part.len() > MAX_RESPONSE {
            return None;
        }
        bytes.extend_from_slice(&part);
    }
    Some(bytes)
}

/// Question `id`'s yes/no verdict in a verdicts response.
pub fn yes_no(response: &Value, id: &str) -> Result<YesNo, String> {
    let verdict = &response["verdicts"][id];
    if verdict.is_null() {
        return Err("The answer has no verdict for the question".into());
    }
    if verdict["answered"] == json!(false) {
        return Err("The model gave no answer".into());
    }
    let yes = verdict["probabilities"]["true"]
        .as_f64()
        .or_else(|| {
            verdict["value"]
                .as_bool()
                .map(|v| if v { 1.0 } else { 0.0 })
        })
        .filter(|p| (0.0..=1.0).contains(p))
        .ok_or("The answer is not a yes/no verdict")?;
    Ok(YesNo {
        model: response["model"]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(160)
            .collect(),
        yes,
        // A service that doesn't say how sure it is is as sure as its answer is far from a coin flip.
        confidence: verdict["confidence"]
            .as_f64()
            .unwrap_or((yes - 0.5).abs() * 2.0)
            .clamp(0.0, 1.0),
    })
}

/// The address questions go to. An origin, or an API base ending in /v1, gets the
/// verdicts path; any other path is used as given, for services that serve the format
/// elsewhere.
pub fn endpoint_url(input: &str) -> Result<Url, String> {
    let input = input.trim();
    let wrong =
        || "Enter the endpoint as an http(s) URL, such as https://gateway.example.com".to_string();
    if input.is_empty() || input.len() > MAX_URL {
        return Err(wrong());
    }
    let mut url = Url::parse(input).map_err(|_| wrong())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(wrong());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Put the key in its own field, not in the URL".into());
    }
    if url.fragment().is_some() {
        return Err("The URL can't have a #fragment".into());
    }
    match url.path().trim_end_matches('/') {
        "" => url.set_path(PATH),
        "/v1" => url.set_path(PATH),
        _ => {}
    }
    Ok(url)
}

/// A change an administrator sends. Fields left out keep their saved values.
#[derive(Default)]
struct Patch {
    endpoint: Option<&'static str>,
    url: Option<Url>,
    /// Some(None) removes the saved key.
    key: Option<Option<String>>,
    model: Option<Option<String>>,
    check_runs: Option<bool>,
    check_tree: Option<bool>,
    check_reports: Option<bool>,
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError::bad(message)
}

fn patch(body: &Value) -> Result<Patch, ApiError> {
    let fields = body.as_object().ok_or_else(|| {
        bad("Send an object with endpoint, url, key, model, checkRuns, checkTree or checkReports")
    })?;
    if let Some(key) = fields.keys().find(|k| {
        ![
            "endpoint",
            "url",
            "key",
            "model",
            "checkRuns",
            "checkTree",
            "checkReports",
        ]
        .contains(&k.as_str())
    }) {
        return Err(bad(format!("Unknown setting: {key}")));
    }
    let mut patch = Patch::default();
    if let Some(value) = fields.get("endpoint") {
        patch.endpoint = Some(match value.as_str() {
            Some("gateway") => "gateway",
            Some("custom") => "custom",
            Some("off") => "off",
            _ => return Err(bad("endpoint must be gateway, custom or off")),
        });
    }
    match fields.get("url") {
        None | Some(Value::Null) => {}
        Some(Value::String(url)) => patch.url = Some(endpoint_url(url).map_err(bad)?),
        Some(_) => return Err(bad("url must be text")),
    }
    match fields.get("key") {
        None => {}
        Some(Value::Null) => patch.key = Some(None),
        Some(Value::String(key))
            if !key.trim().is_empty()
                && key.len() <= MAX_KEY
                && !key.chars().any(char::is_control) =>
        {
            patch.key = Some(Some(key.trim().to_owned()))
        }
        Some(_) => {
            return Err(bad(format!(
                "key must be text of at most {MAX_KEY} characters, or null to remove it"
            )))
        }
    }
    match fields.get("model") {
        None => {}
        Some(Value::Null) => patch.model = Some(None),
        Some(Value::String(model)) if model.trim().is_empty() => patch.model = Some(None),
        Some(Value::String(model)) if valid_model(model.trim()) => {
            patch.model = Some(Some(model.trim().to_owned()))
        }
        Some(_) => {
            return Err(bad(
                "model must be a model name, or null for the endpoint's default",
            ))
        }
    }
    for (name, field) in [
        ("checkRuns", &mut patch.check_runs),
        ("checkTree", &mut patch.check_tree),
        ("checkReports", &mut patch.check_reports),
    ] {
        if let Some(value) = fields.get(name) {
            *field = Some(
                value
                    .as_bool()
                    .ok_or_else(|| bad(format!("{name} must be true or false")))?,
            );
        }
    }
    Ok(patch)
}

impl Patch {
    /// The saved row with this change, the new key not yet sealed. Without a URL a
    /// custom endpoint has nowhere to go.
    fn apply(&self, mut row: Row) -> Result<Row, ApiError> {
        if let Some(endpoint) = self.endpoint {
            row.endpoint = endpoint.into();
        }
        if let Some(url) = &self.url {
            row.url = Some(url.to_string());
        }
        if let Some(None) = self.key {
            row.api_key = None;
        }
        if let Some(model) = &self.model {
            row.model = model.clone();
        }
        if let Some(check_runs) = self.check_runs {
            row.check_runs = check_runs;
        }
        if let Some(check_tree) = self.check_tree {
            row.check_tree = check_tree;
        }
        if let Some(check_reports) = self.check_reports {
            row.check_reports = check_reports;
        }
        if row.endpoint == "custom" && row.url.is_none() {
            return Err(bad("Enter the endpoint's URL"));
        }
        Ok(row)
    }

    fn new_key(&self) -> Option<&str> {
        self.key.as_ref()?.as_deref()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/admin/decisions", get(read).patch(save))
        .route("/api/admin/decisions/check", post(check))
        .with_state(state)
}

async fn document(s: &AppState, user: &Identity) -> Result<Value, ApiError> {
    let d = &s.decisions;
    let row = d.row().await?;
    let settings = d.resolve(&row, None);
    // The gateway's models, for suggestions; another endpoint may serve others.
    let models = match &s.gateway {
        Some(gateway) => gateway.models(user).await.ok().map(|m| {
            m["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["id"].as_str())
                .filter(|id| *id != "auto")
                .map(str::to_owned)
                .collect::<Vec<_>>()
        }),
        None => None,
    };
    let key = match &settings.endpoint {
        Endpoint::Custom { key, .. } => key.clone(),
        _ => match &row.api_key {
            Some(sealed) => d.open(sealed).map_or(Key::Unreadable, Key::Set),
            None => Key::None,
        },
    };
    Ok(json!({
        "endpoint": row.endpoint,
        "url": row.url,
        "hasKey": key != Key::None,
        "keyUnreadable": key == Key::Unreadable,
        "model": row.model,
        "checkRuns": row.check_runs,
        "checkTree": row.check_tree,
        "checkReports": row.check_reports,
        "canStoreKey": d.secret.is_some(),
        "gateway": s.gateway.is_some(),
        "models": models,
    }))
}

async fn read(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, false).await?;
    Ok(Json(document(&s, &user).await?))
}

async fn save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, true).await?;
    let patch = patch(&body)?;
    let sealed = match patch.new_key() {
        Some(key) => {
            let secret = s.decisions.secret.as_ref().ok_or_else(|| {
                bad("Saving a key needs OBLETH_CHAT_SECRET_KEY_FILE, which encrypts it")
            })?;
            Some(secret.seal(KEY_FORMAT, KEY_AAD, key.as_bytes()))
        }
        None => None,
    };
    let mut tx = crate::operations::admin_transaction(&s.store.pool, user.id).await?;
    let row: Row = sqlx::query_as(
        "SELECT endpoint, url, api_key, model, check_runs, check_tree, check_reports FROM decisions WHERE id FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await?;
    let mut row = patch.apply(row)?;
    if sealed.is_some() {
        row.api_key = sealed;
    }
    sqlx::query(
        "UPDATE decisions SET endpoint=$1, url=$2, api_key=$3, model=$4, check_runs=$5, check_tree=$6, check_reports=$7, updated_at=NOW() WHERE id",
    )
    .bind(&row.endpoint)
    .bind(&row.url)
    .bind(&row.api_key)
    .bind(&row.model)
    .bind(row.check_runs)
    .bind(row.check_tree)
    .bind(row.check_reports)
    .execute(&mut *tx)
    .await?;
    // The key itself never reaches the audit log; only that it changed.
    sqlx::query("INSERT INTO audit_events(id,actor_id,action,target_type,metadata)VALUES($1,$2,'decisions.update','site',$3)")
        .bind(Uuid::new_v4())
        .bind(user.id)
        .bind(json!({"endpoint": row.endpoint, "url": row.url, "model": row.model, "checkRuns": row.check_runs, "checkTree": row.check_tree, "checkReports": row.check_reports, "keyChanged": patch.key.is_some(), "hasKey": row.api_key.is_some()}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(document(&s, &user).await?))
}

/// Asks one question with the settings shown in the form, saved or not, as the
/// administrator, and says what came back.
async fn check(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require_admin(&headers, true).await?;
    let patch = patch(&body)?;
    let row = patch.apply(s.decisions.row().await?)?;
    let settings = s.decisions.resolve(&row, patch.new_key());
    let began = Instant::now();
    let asked = s
        .decisions
        .ask(
            &settings,
            s.gateway.as_ref(),
            &user,
            json!("Water boils at 100 degrees Celsius at sea level."),
            json!({"true_statement": {"type": "boolean", "instructions": "Is this statement true?"}}),
            CHECK_TIMEOUT,
        )
        .await
        .and_then(|response| yes_no(&response, "true_statement"));
    let ms = began.elapsed().as_millis() as u64;
    Ok(Json(match asked {
        Ok(v) => {
            json!({"ok": true, "model": v.model, "yes": v.yes, "confidence": v.confidence, "ms": ms})
        }
        Err(error) => json!({"ok": false, "error": error, "ms": ms}),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_and_api_bases_get_the_verdicts_path() {
        for (given, expected) in [
            (
                "https://gw.example.com",
                "https://gw.example.com/v1/verdicts",
            ),
            (
                "https://gw.example.com/",
                "https://gw.example.com/v1/verdicts",
            ),
            (
                "http://gw.internal:8080/v1",
                "http://gw.internal:8080/v1/verdicts",
            ),
            (
                "https://gw.example.com/v1/",
                "https://gw.example.com/v1/verdicts",
            ),
            (
                "https://d.example.com/api/decide",
                "https://d.example.com/api/decide",
            ),
        ] {
            assert_eq!(endpoint_url(given).unwrap().as_str(), expected, "{given}");
        }
        for wrong in [
            "",
            "gw.example.com",
            "ftp://gw.example.com",
            "https://user:pass@gw.example.com",
            "https://gw.example.com/#x",
        ] {
            assert!(endpoint_url(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn yes_no_reads_the_probability_of_yes() {
        let response = json!({"model": "judge-31b", "verdicts": {"tested": {"type": "boolean", "value": false, "probabilities": {"true": 0.125, "false": 0.875}, "confidence": 0.6}}});
        assert_eq!(
            yes_no(&response, "tested").unwrap(),
            YesNo {
                model: "judge-31b".into(),
                yes: 0.125,
                confidence: 0.6
            }
        );
        // A service that sends only the value still gives a usable answer.
        let bare = json!({"verdicts": {"tested": {"value": false}}});
        let found = yes_no(&bare, "tested").unwrap();
        assert_eq!((found.yes, found.confidence), (0.0, 1.0));
        assert!(found.sure_no());
        let unsure = json!({"verdicts": {"tested": {"probabilities": {"true": 0.4, "false": 0.6}, "confidence": 0.02}}});
        assert!(!yes_no(&unsure, "tested").unwrap().sure_no());
        let unanswered = json!({"verdicts": {"tested": {"value": true, "probabilities": {"true": 0.5, "false": 0.5}, "confidence": 0.0, "answered": false}}});
        assert!(yes_no(&unanswered, "tested").is_err());
        assert!(yes_no(&json!({"verdicts": {}}), "tested").is_err());
        let out_of_range = json!({"verdicts": {"tested": {"probabilities": {"true": 1.5}}}});
        assert!(yes_no(&out_of_range, "tested").is_err());
    }

    #[test]
    fn patches_take_only_known_settings() {
        let p = patch(&json!({"endpoint": "custom", "url": "https://d.example.com", "key": " sk-1 ", "model": "", "checkRuns": false, "checkTree": false, "checkReports": true})).unwrap();
        assert_eq!(p.endpoint, Some("custom"));
        assert_eq!(p.url.unwrap().as_str(), "https://d.example.com/v1/verdicts");
        assert_eq!(p.key, Some(Some("sk-1".into())));
        assert_eq!(p.model, Some(None));
        assert_eq!(p.check_runs, Some(false));
        assert_eq!((p.check_tree, p.check_reports), (Some(false), Some(true)));
        assert_eq!(patch(&json!({"key": null})).unwrap().key, Some(None));
        for wrong in [
            json!({"endpoint": "elsewhere"}),
            json!({"url": "nope"}),
            json!({"key": ""}),
            json!({"model": "a\nb"}),
            json!({"checkRuns": "yes"}),
            json!({"checkTree": 1}),
            json!({"secret": 1}),
            json!([]),
        ] {
            assert!(patch(&wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn a_custom_endpoint_needs_a_url() {
        let row = || Row {
            endpoint: "gateway".into(),
            url: None,
            api_key: Some(vec![1]),
            model: None,
            check_runs: true,
            check_tree: true,
            check_reports: true,
        };
        assert!(patch(&json!({"endpoint": "custom"}))
            .unwrap()
            .apply(row())
            .is_err());
        let set = patch(&json!({"endpoint": "custom", "url": "https://d.example.com"}))
            .unwrap()
            .apply(row())
            .unwrap();
        assert_eq!(
            set.url.as_deref(),
            Some("https://d.example.com/v1/verdicts")
        );
        // Switching away keeps the URL and key, so switching back restores them.
        let off = patch(&json!({"endpoint": "off"}))
            .unwrap()
            .apply(set)
            .unwrap();
        assert_eq!(off.endpoint, "off");
        assert!(off.url.is_some() && off.api_key.is_some());
        assert!(patch(&json!({"key": null}))
            .unwrap()
            .apply(off)
            .unwrap()
            .api_key
            .is_none());
    }

    #[test]
    fn debug_never_shows_the_key() {
        let endpoint = Endpoint::Custom {
            url: endpoint_url("https://d.example.com").unwrap(),
            key: Key::Set("sk-secret".into()),
        };
        let shown = format!("{endpoint:?}");
        assert!(!shown.contains("sk-secret"));
        assert!(shown.contains("set"));
    }
}
