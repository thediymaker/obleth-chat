use crate::{
    auth::{grants::Grants, Identity},
    error::ApiError,
};
use anyhow::{bail, Context, Result};
use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::{Client, Response};
use rsa::{
    pkcs1::DecodeRsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts, RsaPrivateKey,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
pub struct Gateway {
    client: Client,
    base: Url,
    /// Signs Chat's own token. Optional when people's provider tokens are forwarded;
    /// it then serves only local accounts.
    signer: Option<Arc<Signer>>,
    /// People's provider tokens (OBLETH_CHAT_GATEWAY_AUTH=forward).
    grants: Option<Grants>,
    /// Each person's model catalog for CATALOG_TTL, fetched once for everyone asking.
    catalogs: Arc<Mutex<HashMap<Uuid, (Instant, Catalog)>>>,
    /// Chat's signed token for each person, reused until it nears expiry.
    tokens: Arc<Mutex<HashMap<Uuid, Token>>>,
}

/// How long a person's model catalog is reused. Models added or removed on the
/// gateway reach Chat at most this late; the admin model list always asks afresh.
const CATALOG_TTL: Duration = Duration::from_secs(15);
/// How long Chat's signed tokens last, and how much of that is left when a cached
/// one is replaced.
const TOKEN_SECS: u64 = 300;
const TOKEN_RENEW_SECS: u64 = 60;
/// How long connecting to the gateway may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the model catalog and its details may take.
const CATALOG_TIMEOUT: Duration = Duration::from_secs(15);
/// How long `/v1/load` may take; it only decorates the model list.
const LOAD_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an embeddings request may take.
const EMBEDDINGS_TIMEOUT: Duration = Duration::from_secs(120);
/// How long the reason in a failed answer is waited for.
const ERROR_BODY_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest model catalog or model details answer read.
const MAX_CATALOG: usize = 2 * 1024 * 1024;
/// The largest `/v1/load` answer read.
const MAX_LOAD: usize = 1024 * 1024;
/// The largest whole completion or verdict read.
const MAX_ANSWER: usize = 16 * 1024 * 1024;
/// The most an embeddings answer may take per input: room for 8192 floats written
/// out in full, well above the dimensions models produce.
const MAX_EMBEDDING: usize = 256 * 1024;

type Catalog = Arc<tokio::sync::OnceCell<Vec<DiscoveredModel>>>;

/// A signed token and what it was signed for. It carries no per-request claims.
struct Token {
    username: String,
    value: String,
    expires: u64,
}

/// For Auto, a thinking effort other than the default tells the gateway's router how
/// hard the request is; at the default the router judges that itself.
fn effort_header(body: &Value) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    if body.get("model").and_then(Value::as_str) == Some("auto") {
        let effort = match body.get("reasoning_effort").and_then(Value::as_str) {
            Some("none" | "low") => Some("low"),
            Some("high") => Some("high"),
            _ => None,
        };
        if let Some(effort) = effort {
            headers.insert(
                "x-obleth-effort",
                reqwest::header::HeaderValue::from_static(effort),
            );
        }
    }
    headers
}

/// Deliberately allowlisted discovery fields. Never return upstream routing,
/// credentials, arbitrary provider parameters, or raw model-info objects.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMetadata {
    pub mode: String,
    pub context_window: Option<u64>,
    pub supports_vision: Option<bool>,
    pub supports_function_calling: Option<bool>,
    pub supports_reasoning: Option<bool>,
    pub healthy: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct DiscoveredModel {
    pub id: String,
    pub tags: Vec<String>,
    pub metadata: ModelMetadata,
}
struct Signer {
    key: EncodingKey,
    kid: String,
    issuer: String,
    audience: String,
    jwks: Value,
}
#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    aud: &'a str,
    sub: String,
    username: &'a str,
    iat: u64,
    nbf: u64,
    exp: u64,
    jti: String,
}

impl Gateway {
    pub fn new(base: Url, pem: &[u8], issuer: String, audience: String) -> Result<Self> {
        let text = std::str::from_utf8(pem).context("Signing key must be a PEM file")?;
        let private = RsaPrivateKey::from_pkcs8_pem(text)
            .or_else(|_| RsaPrivateKey::from_pkcs1_pem(text))
            .context("Invalid RSA signing key")?;
        if private.n().bits() < 2048 {
            bail!("Gateway signing key must be at least 2048 bits");
        }
        let n = URL_SAFE_NO_PAD.encode(private.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(private.e().to_bytes_be());
        let kid = URL_SAFE_NO_PAD.encode(Sha256::digest(format!("{n}.{e}").as_bytes()));
        let jwks = json!({"keys":[{"kty":"RSA","use":"sig","alg":"RS256","kid":kid,"n":n,"e":e}]});
        let key = EncodingKey::from_rsa_pem(pem).context("Invalid RSA signing key")?;
        let mut gateway = Self::forwarding(base)?;
        gateway.signer = Some(Arc::new(Signer {
            key,
            kid,
            issuer,
            audience,
            jwks,
        }));
        Ok(gateway)
    }
    /// A gateway without Chat's signing key, for forwarding people's own tokens;
    /// add them with `forward_tokens`.
    pub fn forwarding(base: Url) -> Result<Self> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            base,
            signer: None,
            grants: None,
            catalogs: Arc::default(),
            tokens: Arc::default(),
        })
    }
    /// Sends each person who signed in with the identity provider to the gateway
    /// with their own provider access token.
    pub fn forward_tokens(mut self, grants: Grants) -> Self {
        self.grants = Some(grants);
        self
    }
    pub async fn from_file(
        base: Url,
        file: &Path,
        issuer: String,
        audience: String,
    ) -> Result<Self> {
        Self::new(
            base,
            &tokio::fs::read(file)
                .await
                .context("Cannot read gateway signing key")?,
            issuer,
            audience,
        )
    }
    /// Chat's public signing keys; empty without a signing key.
    pub fn jwks(&self) -> Value {
        match &self.signer {
            Some(signer) => signer.jwks.clone(),
            None => json!({"keys":[]}),
        }
    }
    pub fn signs(&self) -> bool {
        self.signer.is_some()
    }
    /// The HTTP client gateway calls share.
    pub(crate) fn http(&self) -> &Client {
        &self.client
    }
    /// A gateway path, such as `v1/batches`, as a full URL.
    pub(crate) fn url(&self, path: &str) -> Url {
        self.base.join(path).unwrap_or_else(|_| self.base.clone())
    }
    /// Sends a request made for `user` with its Authorization header. A request
    /// that doesn't reach the gateway is `unavailable`; the status is the caller's.
    pub(crate) async fn send(
        &self,
        user: &Identity,
        request: reqwest::RequestBuilder,
    ) -> Result<Response, ApiError> {
        request
            .header(
                reqwest::header::AUTHORIZATION,
                self.authorization(user).await?,
            )
            .send()
            .await
            .map_err(|_| unavailable())
    }
    /// The Authorization header for a call made for `user`. Someone who signs in
    /// with the identity provider gets their own provider token whenever tokens are
    /// forwarded, never Chat's; everyone else gets Chat's signed token.
    async fn authorization(
        &self,
        user: &Identity,
    ) -> Result<reqwest::header::HeaderValue, ApiError> {
        if let Some(grants) = &self.grants {
            if let Some(token) = grants.access_token(user.id, user.session).await? {
                return token.authorization();
            }
        }
        let signer = self.signer.as_ref().ok_or_else(|| {
            ApiError(
                StatusCode::FORBIDDEN,
                "This account can't use the models here. Sign in with single sign-on instead."
                    .into(),
            )
        })?;
        let mut value = reqwest::header::HeaderValue::from_str(&format!(
            "Bearer {}",
            self.token(signer, user, now()?)?
        ))
        .map_err(|_| ApiError::internal())?;
        value.set_sensitive(true);
        Ok(value)
    }
    /// Chat's token for `user`: the cached one while it has more than
    /// TOKEN_RENEW_SECS left, otherwise a newly signed one.
    fn token(&self, signer: &Signer, user: &Identity, now: u64) -> Result<String, ApiError> {
        if let Some(token) = self.tokens.lock().unwrap().get(&user.id) {
            if token.username == user.username && token.expires > now + TOKEN_RENEW_SECS {
                return Ok(token.value.clone());
            }
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(signer.kid.clone());
        let expires = now + TOKEN_SECS;
        let value = jsonwebtoken::encode(
            &header,
            &Claims {
                iss: &signer.issuer,
                aud: &signer.audience,
                sub: user.id.to_string(),
                username: &user.username,
                iat: now,
                nbf: now.saturating_sub(5),
                exp: expires,
                jti: Uuid::new_v4().to_string(),
            },
            &signer.key,
        )
        .map_err(|_| ApiError::internal())?;
        let mut tokens = self.tokens.lock().unwrap();
        tokens.retain(|_, token| token.expires > now);
        tokens.insert(
            user.id,
            Token {
                username: user.username.clone(),
                value: value.clone(),
                expires,
            },
        );
        Ok(value)
    }
    /// The ids of every model the gateway lists for `user`, asked afresh, without
    /// `auto`.
    pub async fn model_ids(&self, user: &Identity) -> Result<Vec<String>, ApiError> {
        let catalog = self
            .discovery_json(user, "v1/models", false)
            .await?
            .ok_or_else(unavailable)?;
        let list = catalog
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(unavailable)?;
        Ok(list
            .iter()
            .take(1000)
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .filter(|id| valid_model(id) && *id != "auto")
            .map(str::to_owned)
            .collect())
    }
    /// Live load per model from the gateway's `/v1/load`, as the user. None when the
    /// gateway doesn't offer it: another kind of gateway, an older one, or one whose
    /// administrator turned it off.
    pub async fn load(&self, user: &Identity) -> Result<Option<Value>, ApiError> {
        let response = self
            .send(
                user,
                self.client.get(self.url("v1/load")).timeout(LOAD_TIMEOUT),
            )
            .await?;
        let status = response.status().as_u16();
        if (400..500).contains(&status) && !matches!(status, 401 | 403 | 429) {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(upstream_error(response).await);
        }
        json_body(response, MAX_LOAD).await.map(Some)
    }
    /// Embeds each input with the given model, as the user, in input order.
    pub async fn embeddings(
        &self,
        user: &Identity,
        model: &str,
        input: &[String],
    ) -> Result<Vec<Vec<f32>>, ApiError> {
        let request = self
            .client
            .post(self.url("v1/embeddings"))
            .timeout(EMBEDDINGS_TIMEOUT)
            .json(&json!({"model":model,"input":input,"encoding_format":"float"}));
        let response = self.send(user, request).await?;
        if !response.status().is_success() {
            return Err(upstream_error(response).await);
        }
        let data = json_body(response, MAX_EMBEDDING * input.len().max(1)).await?;
        let mut rows: Vec<(u64, Vec<f32>)> = data["data"]
            .as_array()
            .ok_or_else(unavailable)?
            .iter()
            .map(|row| {
                let index = row["index"].as_u64().unwrap_or(0);
                let vector = row["embedding"]
                    .as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(Value::as_f64)
                            .map(|x| x as f32)
                            .collect()
                    })
                    .unwrap_or_default();
                (index, vector)
            })
            .collect();
        rows.sort_by_key(|(index, _)| *index);
        if rows.len() != input.len() || rows.iter().any(|(_, v)| v.is_empty()) {
            return Err(unavailable());
        }
        Ok(rows.into_iter().map(|(_, v)| v).collect())
    }
    /// Asks the gateway's /v1/verdicts, as the user.
    pub async fn verdicts(
        &self,
        user: &Identity,
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, ApiError> {
        let request = self.client.post(self.url("v1/verdicts")).timeout(timeout);
        let response = self.send(user, request.json(body)).await?;
        if !response.status().is_success() {
            return Err(upstream_error(response).await);
        }
        json_body(response, MAX_ANSWER).await
    }
    /// A whole completion in one answer, for work Chat does on its own, such as naming
    /// the task a chat was about.
    pub async fn complete(
        &self,
        user: &Identity,
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, ApiError> {
        let request = self
            .client
            .post(self.url("v1/chat/completions"))
            .timeout(timeout);
        let response = self.send(user, request.json(body)).await?;
        if !response.status().is_success() {
            return Err(upstream_error(response).await);
        }
        json_body(response, MAX_ANSWER).await
    }
    pub async fn chat(
        &self,
        user: &Identity,
        conversation: Uuid,
        body: &Value,
    ) -> Result<Response, ApiError> {
        self.chat_escalating(user, conversation, body, None, None)
            .await
            .map_err(ChatFailure::into_error)
    }
    /// A chat request that can tell `auto` the model it gave this conversation
    /// is failing (`x-obleth-auto-escalate`, such as `tool_loop`), so the
    /// gateway moves it to a stronger one. `own_system` is how many bytes at the
    /// end of the system message the person wrote; `auto` classifies the request
    /// from those and the person's turns only (`x-obleth-classify-system`).
    pub async fn chat_escalating(
        &self,
        user: &Identity,
        conversation: Uuid,
        body: &Value,
        escalate: Option<&'static str>,
        own_system: Option<usize>,
    ) -> Result<Response, ChatFailure> {
        let mut extra = effort_header(body);
        if let Some(signal) = escalate.filter(|_| body["model"] == "auto") {
            extra.insert(
                "x-obleth-auto-escalate",
                reqwest::header::HeaderValue::from_static(signal),
            );
        }
        if let Some(bytes) = own_system.filter(|_| body["model"] == "auto") {
            extra.insert("x-obleth-classify-system", bytes.into());
        }
        let authorization = self
            .authorization(user)
            .await
            .map_err(ChatFailure::Refused)?;
        let response = self
            .client
            .post(self.url("v1/chat/completions"))
            .header(reqwest::header::AUTHORIZATION, authorization)
            .header("x-obleth-session-id", conversation.to_string())
            .headers(extra)
            .json(body)
            .send()
            .await
            .map_err(|_| ChatFailure::Transient(unavailable()))?;
        if !response.status().is_success() {
            let failed = response.status().is_server_error();
            let error = upstream_error(response).await;
            return Err(if failed && error.1 != CAPACITY_TIMEOUT {
                ChatFailure::Transient(error)
            } else {
                ChatFailure::Refused(error)
            });
        }
        if !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"))
        {
            return Err(ChatFailure::Refused(ApiError(
                StatusCode::BAD_GATEWAY,
                "Gateway did not return a streaming response".into(),
            )));
        }
        Ok(response)
    }

    async fn discovery_json(
        &self,
        user: &Identity,
        path: &str,
        optional: bool,
    ) -> Result<Option<Value>, ApiError> {
        let request = self.client.get(self.url(path)).timeout(CATALOG_TIMEOUT);
        let response = self.send(user, request).await?;
        if optional && matches!(response.status().as_u16(), 404 | 405 | 501) {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(upstream_error(response).await);
        }
        json_body(response, MAX_CATALOG).await.map(Some)
    }

    /// The chat models the gateway offers `user`, reused for CATALOG_TTL; concurrent
    /// callers share one fetch. `fresh` asks the gateway again and keeps the answer.
    pub async fn discover_models(
        &self,
        user: &Identity,
        fresh: bool,
    ) -> Result<Vec<DiscoveredModel>, ApiError> {
        let catalog = {
            let mut catalogs = self.catalogs.lock().unwrap();
            let now = Instant::now();
            catalogs.retain(|_, (at, _)| now.duration_since(*at) < CATALOG_TTL);
            if fresh {
                catalogs.remove(&user.id);
            }
            catalogs
                .entry(user.id)
                .or_insert_with(|| (now, Catalog::default()))
                .1
                .clone()
        };
        catalog
            .get_or_try_init(|| self.fetch_models(user))
            .await
            .cloned()
    }

    /// Drops every cached model list, so the next request asks the gateway.
    pub fn forget_models(&self) {
        self.catalogs.lock().unwrap().clear();
    }

    async fn fetch_models(&self, user: &Identity) -> Result<Vec<DiscoveredModel>, ApiError> {
        let (catalog, details) = tokio::try_join!(
            self.discovery_json(user, "v1/models", false),
            self.discovery_json(user, "model/info", true),
        )?;
        let catalog = catalog.ok_or_else(unavailable)?;
        let list = catalog
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(unavailable)?;
        let details = match details.as_ref() {
            Some(value) => Some(
                value
                    .get("data")
                    .and_then(Value::as_array)
                    .ok_or_else(unavailable)?,
            ),
            None => None,
        };
        let mut models = Vec::new();
        for entry in list.iter().take(1000) {
            let Some(id) = entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| valid_model(id) && *id != "auto")
            else {
                continue;
            };
            if models.iter().any(|model: &DiscoveredModel| model.id == id) {
                continue;
            }
            let info = details
                .and_then(|list| {
                    list.iter()
                        .find(|detail| detail.get("model_name").and_then(Value::as_str) == Some(id))
                })
                .and_then(|v| v.get("model_info"));
            // Generic OpenAI catalogs do not declare mode. Known non-chat modes
            // are excluded; metadata never expands the authoritative ID listing.
            let mode = entry
                .get("mode")
                .and_then(Value::as_str)
                .or_else(|| info.and_then(|v| v.get("mode")).and_then(Value::as_str));
            if mode.is_some_and(|mode| mode != "chat") {
                continue;
            }
            let model_type = entry.get("model_type").and_then(Value::as_str).or_else(|| {
                info.and_then(|v| v.get("model_type"))
                    .and_then(Value::as_str)
            });
            if model_type.is_some_and(|kind| {
                matches!(
                    kind,
                    "embedding"
                        | "embeddings"
                        | "rerank"
                        | "reranker"
                        | "image"
                        | "image_generation"
                        | "audio"
                        | "tts"
                        | "stt"
                        | "transcription"
                        | "speech"
                        | "video"
                )
            }) {
                continue;
            }
            let tags = entry
                .get("tags")
                .or_else(|| info.and_then(|v| v.get("tags")))
                .and_then(Value::as_array)
                .map(|tags| {
                    tags.iter()
                        .filter_map(Value::as_str)
                        .filter(|s| s.len() <= 32 && !s.chars().any(char::is_control))
                        .take(12)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let bool_field = |key| info.and_then(|v| v.get(key)).and_then(Value::as_bool);
            let context_window = info
                .and_then(|v| {
                    v.get("context_window")
                        .or_else(|| v.get("max_input_tokens"))
                })
                .and_then(Value::as_u64)
                .filter(|n| *n > 0 && *n <= 100_000_000);
            models.push(DiscoveredModel {
                id: id.into(),
                tags,
                metadata: ModelMetadata {
                    mode: mode.unwrap_or("chat").to_owned(),
                    context_window,
                    supports_vision: takes_images(info),
                    supports_function_calling: bool_field("supports_function_calling"),
                    supports_reasoning: bool_field("supports_reasoning"),
                    healthy: bool_field("healthy"),
                },
            });
        }
        Ok(models)
    }

    /// Models the gateway lists as embedding models, for choosing Knowledge's model.
    pub async fn embedding_models(&self, user: &Identity) -> Result<Vec<String>, ApiError> {
        let (catalog, details) = tokio::try_join!(
            self.discovery_json(user, "v1/models", false),
            self.discovery_json(user, "model/info", true),
        )?;
        let catalog = catalog.ok_or_else(unavailable)?;
        let list = catalog
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(unavailable)?;
        let details = details
            .as_ref()
            .and_then(|value| value.get("data"))
            .and_then(Value::as_array);
        let mut models = Vec::new();
        for entry in list.iter().take(1000) {
            let Some(id) = entry
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| valid_model(id))
            else {
                continue;
            };
            let info = details
                .and_then(|list| {
                    list.iter()
                        .find(|detail| detail.get("model_name").and_then(Value::as_str) == Some(id))
                })
                .and_then(|v| v.get("model_info"));
            let field = |key| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .or_else(|| info.and_then(|v| v.get(key)).and_then(Value::as_str))
            };
            let embedding = field("mode") == Some("embedding")
                || matches!(field("model_type"), Some("embedding" | "embeddings"));
            if embedding && !models.iter().any(|m| m == id) {
                models.push(id.to_owned());
            }
        }
        models.sort();
        Ok(models)
    }
}
/// Seconds since the epoch.
fn now() -> Result<u64, ApiError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ApiError::internal())?
        .as_secs())
}

pub fn valid_model(model: &str) -> bool {
    !model.is_empty() && model.len() <= 160 && !model.chars().any(char::is_control)
}
pub fn unavailable() -> ApiError {
    ApiError(
        StatusCode::BAD_GATEWAY,
        "The gateway could not complete this request".into(),
    )
}
/// Why a chat request got no stream.
#[derive(Debug)]
pub enum ChatFailure {
    /// The request did not reach the gateway, or the gateway failed (a 5xx other
    /// than giving up on capacity): worth sending again.
    Transient(ApiError),
    /// Refused, rate limited, out of capacity, or not a stream: sending it again
    /// would not help.
    Refused(ApiError),
}

impl ChatFailure {
    pub fn into_error(self) -> ApiError {
        match self {
            ChatFailure::Transient(error) | ChatFailure::Refused(error) => error,
        }
    }
}

/// A gateway answer's JSON body, refusing one larger than `max`.
async fn json_body(response: Response, max: usize) -> Result<Value, ApiError> {
    let bytes = crate::http::read_capped(response, max)
        .await
        .map_err(|_| unavailable())?;
    serde_json::from_slice(&bytes).map_err(|_| unavailable())
}
/// What a reply says when the gateway stopped waiting for a free slot on its model.
pub const CAPACITY_TIMEOUT: &str =
    "The model stayed full, so the gateway stopped waiting. Retry, or pick another model.";
/// Whether an obleth gateway gave up on a request that waited its whole queue time for a slot.
fn gave_up_waiting(status: u16, reason: Option<&str>) -> bool {
    status == 503 && reason.is_some_and(|r| r.contains("waiting for model capacity"))
}
/// The gateway's reason, when it gave one, follows Chat's own words, so a person can
/// tell a retired model from a bad request. It is cut short and kept to one line.
pub(crate) async fn upstream_error(response: Response) -> ApiError {
    let status = response.status();
    tracing::warn!(status = status.as_u16(), "gateway request failed");
    let message = match status.as_u16() {
        401 => "Gateway rejected the Chat identity. Check its trusted issuer configuration.",
        403 => "Your gateway identity does not have access to this model.",
        429 => "Your gateway quota or queue limit was reached. Try again later.",
        _ => "The gateway could not complete this request",
    };
    let reason = match status.as_u16() {
        401 | 403 => None,
        _ => tokio::time::timeout(ERROR_BODY_TIMEOUT, response.bytes())
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|body| reason(&body)),
    };
    if gave_up_waiting(status.as_u16(), reason.as_deref()) {
        return ApiError(StatusCode::SERVICE_UNAVAILABLE, CAPACITY_TIMEOUT.into());
    }
    ApiError(
        if status.as_u16() == 429 {
            StatusCode::TOO_MANY_REQUESTS
        } else {
            StatusCode::BAD_GATEWAY
        },
        match reason {
            Some(reason) => format!("{message}: {reason}"),
            None => message.into(),
        },
    )
}
/// `error.message` (or a string `error`) from an OpenAI-style error body.
pub(crate) fn reason(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(&body[..body.len().min(64 * 1024)]).ok()?;
    let text = value["error"]["message"]
        .as_str()
        .or_else(|| value["error"].as_str())
        .or_else(|| value["message"].as_str())?;
    let line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(300)
        .collect();
    Some(line.trim().trim_end_matches('.').to_owned()).filter(|l| !l.is_empty())
}

/// Only provider-supplied text belongs in the visible reasoning channel. Prefer
/// the normalized spelling when aliases are mirrored, and never stringify
/// metadata, signatures, encrypted reasoning, or arbitrary object fields.
pub fn text_deltas(choice: &Value) -> Vec<(&'static str, &str)> {
    let mut text = Vec::with_capacity(2);
    if let Some(delta) = choice.get("delta") {
        if let Some(reasoning) = ["reasoning_content", "reasoning"]
            .into_iter()
            .find_map(|key| {
                delta
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
            })
        {
            text.push(("reasoning", reasoning));
        }
        if let Some(content) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            text.push(("delta", content));
        }
    }
    text
}

/// The largest gateway event. Generated images arrive in one content event as data
/// URLs (the gateway attaches up to 3 MiB), so this is well above that.
pub const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

/// Incremental, bounded SSE parser. Keeps incomplete UTF-8 in bytes until a full line arrives.
#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    data: String,
    event_bytes: usize,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, &'static str> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > MAX_EVENT_BYTES {
            return Err("Gateway event exceeded the size limit");
        }
        let mut events = Vec::new();
        let mut consumed = 0;
        while let Some(end) = self.buffer[consumed..].iter().position(|b| *b == b'\n') {
            let start = consumed;
            consumed += end + 1;
            let line = std::str::from_utf8(&self.buffer[start..consumed - 1])
                .map_err(|_| "Gateway returned invalid UTF-8")?
                .trim_end_matches('\r');
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(std::mem::take(&mut self.data));
                }
                self.event_bytes = 0;
            } else {
                self.event_bytes += line.len();
                if self.event_bytes > MAX_EVENT_BYTES {
                    return Err("Gateway event exceeded the size limit");
                }
                if let Some(data) = line.strip_prefix("data:") {
                    if !self.data.is_empty() {
                        self.data.push('\n');
                    }
                    self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
                }
            }
        }
        self.buffer.drain(..consumed);
        Ok(events)
    }
}
/// Whether a model takes images: its own vision, or the gateway's vision boon, which
/// describes images for a text-only model (obleth lists it in `boon_details`).
fn takes_images(info: Option<&Value>) -> Option<bool> {
    let native = info
        .and_then(|v| v.get("supports_vision"))
        .and_then(Value::as_bool);
    let lent = info
        .and_then(|v| v.pointer("/boon_details/vision/active"))
        .and_then(Value::as_bool)
        == Some(true);
    if lent {
        Some(true)
    } else {
        native
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(username: &str) -> Identity {
        Identity {
            id: Uuid::from_u128(7),
            username: username.into(),
            display_name: "Test".into(),
            role: crate::auth::Role::User,
            session: None,
        }
    }

    /// A gateway that signs, in front of a catalog that counts its requests.
    async fn counted() -> (Gateway, Arc<std::sync::atomic::AtomicUsize>) {
        use axum::{routing::get, Json, Router};
        let listed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = listed.clone();
        let app = Router::new()
            .route(
                "/v1/models",
                get(move || {
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Json(json!({"data":[{"id":"fixture-model"}]}))
                    }
                }),
            )
            // An embedding as long as the input asks for.
            .route(
                "/v1/embeddings",
                axum::routing::post(|Json(body): Json<Value>| async move {
                    let length: usize = body["input"][0].as_str().unwrap().parse().unwrap();
                    Json(json!({"data":[{"index":0,"embedding":vec![0.123_456_79; length]}]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let pem = {
            use rsa::pkcs8::EncodePrivateKey;
            RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
                .unwrap()
                .to_pkcs8_pem(Default::default())
                .unwrap()
        };
        let gateway = Gateway::new(
            base.parse().unwrap(),
            pem.as_bytes(),
            "http://chat".into(),
            "obleth".into(),
        )
        .unwrap();
        (gateway, listed)
    }

    #[tokio::test]
    async fn an_oversized_embeddings_answer_is_refused() {
        let (gateway, _) = counted().await;
        let user = person("ada");
        let vectors = gateway
            .embeddings(&user, "fixture-embed", &["4096".into()])
            .await
            .unwrap();
        assert_eq!(vectors[0].len(), 4096);
        let error = gateway
            .embeddings(&user, "fixture-embed", &["100000".into()])
            .await
            .unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn the_catalog_is_fetched_once_per_person_within_its_lifetime() {
        let (gateway, listed) = counted().await;
        let user = person("ada");
        let calls: Vec<_> = (0..6)
            .map(|_| gateway.discover_models(&user, false))
            .collect();
        for found in futures_util::future::join_all(calls).await {
            assert_eq!(found.unwrap()[0].id, "fixture-model");
        }
        gateway.discover_models(&user, false).await.unwrap();
        assert_eq!(listed.load(std::sync::atomic::Ordering::SeqCst), 1);
        // The admin list asks again.
        gateway.discover_models(&user, true).await.unwrap();
        assert_eq!(listed.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_signed_token_is_reused_until_it_nears_expiry() {
        let (gateway, _) = counted().await;
        let signer = gateway.signer.clone().unwrap();
        let user = person("ada");
        let first = gateway.token(&signer, &user, 1_000).unwrap();
        assert_eq!(gateway.token(&signer, &user, 1_200).unwrap(), first);
        // Under a minute left: signed again.
        let renewed = gateway.token(&signer, &user, 1_241).unwrap();
        assert_ne!(renewed, first);
        // A renamed account gets a token with its new name.
        assert_ne!(
            gateway.token(&signer, &person("grace"), 1_242).unwrap(),
            renewed
        );
    }

    #[test]
    fn a_model_with_the_vision_boon_takes_images() {
        let boon = json!({"supports_vision":false,"boons":["vision"],"boon_details":{"vision":{"active":true,"native":false,"helper_model":"gemma4-31b-it"}}});
        let off = json!({"supports_vision":false,"boons":["vision"],"boon_details":{"vision":{"active":false,"native":false,"inactive_reason":"off"}}});
        assert_eq!(takes_images(Some(&boon)), Some(true));
        assert_eq!(takes_images(Some(&off)), Some(false));
        assert_eq!(
            takes_images(Some(&json!({"supports_vision":true}))),
            Some(true)
        );
        assert_eq!(
            takes_images(Some(&json!({"supports_vision":false}))),
            Some(false)
        );
        assert_eq!(takes_images(None), None);
    }

    #[test]
    fn a_request_the_gateway_stopped_queueing_is_told_apart() {
        assert!(gave_up_waiting(
            503,
            Some("timed out waiting for model capacity")
        ));
        assert!(!gave_up_waiting(503, Some("no healthy upstream")));
        assert!(!gave_up_waiting(
            429,
            Some("timed out waiting for model capacity")
        ));
        assert!(!gave_up_waiting(503, None));
    }
    #[test]
    fn reasoning_text_aliases_are_not_duplicated_or_stringified() {
        let choice = json!({"delta":{"reasoning_content":"Plan α", "reasoning":"Plan α", "content":"Answer"}});
        assert_eq!(
            text_deltas(&choice),
            vec![("reasoning", "Plan α"), ("delta", "Answer")]
        );
        let choice = json!({"delta":{"reasoning_content":null, "reasoning":"Other provider", "content":null}});
        assert_eq!(text_deltas(&choice), vec![("reasoning", "Other provider")]);
        let choice = json!({"delta":{"reasoning_content":{"secret":"not text"}, "reasoning":[{"text":"not a string"}], "content":{}, "reasoning_details":[{"type":"reasoning.encrypted", "data":"secret"}]}});
        assert!(text_deltas(&choice).is_empty());
    }

    #[test]
    fn fragmented_unicode_reasoning_remains_separate_from_answer() {
        let wire = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Plan α\",\"content\":\"Answer β\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        for byte in wire.as_bytes() {
            events.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(events.len(), 2);
        let event: Value = serde_json::from_str(&events[0]).unwrap();
        assert_eq!(
            text_deltas(&event["choices"][0]),
            vec![("reasoning", "Plan α"), ("delta", "Answer β")]
        );
        assert_eq!(events[1], "[DONE]");
    }
    #[test]
    fn parses_fragmented_unicode_crlf_and_multiple_data_lines() {
        let mut d = Decoder::default();
        let payload = "data: α\r\ndata: β\r\n\r\ndata: [DONE]\n\n".as_bytes();
        let mut out = vec![];
        for b in payload {
            out.extend(d.push(&[*b]).unwrap());
        }
        assert_eq!(out, vec!["α\nβ", "[DONE]"]);
    }
    #[test]
    fn bounds_gateway_events() {
        assert!(Decoder::default()
            .push(&vec![b'x'; MAX_EVENT_BYTES + 1])
            .is_err());
    }
    #[test]
    fn rejects_invalid_utf8() {
        assert!(Decoder::default()
            .push(&[b'd', b'a', b't', b'a', b':', 0xff, b'\n'])
            .is_err());
    }
}
