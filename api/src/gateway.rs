use crate::{auth::Identity, error::ApiError};
use anyhow::{bail, Context, Result};
use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::StreamExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::{Client, Response};
use rsa::{
    pkcs1::DecodeRsaPrivateKey, pkcs8::DecodePrivateKey, traits::PublicKeyParts, RsaPrivateKey,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
pub struct Gateway {
    client: Client,
    base: Url,
    signer: Arc<Signer>,
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
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            base,
            signer: Arc::new(Signer {
                key,
                kid,
                issuer,
                audience,
                jwks,
            }),
        })
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
    pub fn jwks(&self) -> Value {
        self.signer.jwks.clone()
    }
    fn token(&self, user: &Identity) -> Result<String, ApiError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ApiError::internal())?
            .as_secs();
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.signer.kid.clone());
        jsonwebtoken::encode(
            &header,
            &Claims {
                iss: &self.signer.issuer,
                aud: &self.signer.audience,
                sub: user.id.to_string(),
                username: &user.username,
                iat: now,
                nbf: now.saturating_sub(5),
                exp: now + 300,
                jti: Uuid::new_v4().to_string(),
            },
            &self.signer.key,
        )
        .map_err(|_| ApiError::internal())
    }
    pub async fn models(&self, user: &Identity) -> Result<Value, ApiError> {
        let response = self
            .client
            .get(self.base.join("v1/models").unwrap())
            .bearer_auth(self.token(user)?)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(upstream_error(response).await);
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(part) = stream.next().await {
            let part = part.map_err(|_| unavailable())?;
            if bytes.len() + part.len() > 1024 * 1024 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&part);
        }
        let data: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        let mut models =
            vec![json!({"id":"auto","name":"Auto","detail":"Let Obleth choose a model"})];
        let Some(list) = data.get("data").and_then(Value::as_array) else {
            return Err(unavailable());
        };
        for m in list.iter().take(1000) {
            if let Some(id) = m
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| valid_model(id) && *id != "auto")
            {
                models.push(
                    json!({"id":id,"name":id,"detail":"Available through your gateway identity"}),
                );
            }
        }
        Ok(json!({"models":models}))
    }
    /// Embeds each input with the given model, as the user, in input order.
    pub async fn embeddings(
        &self,
        user: &Identity,
        model: &str,
        input: &[String],
    ) -> Result<Vec<Vec<f32>>, ApiError> {
        let response = self
            .client
            .post(self.base.join("v1/embeddings").unwrap())
            .bearer_auth(self.token(user)?)
            .timeout(std::time::Duration::from_secs(120))
            .json(&json!({"model":model,"input":input,"encoding_format":"float"}))
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(upstream_error(response).await);
        }
        let data: Value = response.json().await.map_err(|_| unavailable())?;
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
    pub async fn chat(
        &self,
        user: &Identity,
        conversation: Uuid,
        body: Value,
    ) -> Result<Response, ApiError> {
        let response = self
            .client
            .post(self.base.join("v1/chat/completions").unwrap())
            .bearer_auth(self.token(user)?)
            .header("x-obleth-session-id", conversation.to_string())
            .headers(effort_header(&body))
            .json(&body)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success() {
            return Err(upstream_error(response).await);
        }
        if !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"))
        {
            return Err(ApiError(
                StatusCode::BAD_GATEWAY,
                "Gateway did not return a streaming response".into(),
            ));
        }
        Ok(response)
    }

    async fn discovery_json(
        &self,
        user: &Identity,
        path: &str,
        optional: bool,
    ) -> Result<Option<Value>, ApiError> {
        let response = self
            .client
            .get(self.base.join(path).unwrap())
            .bearer_auth(self.token(user)?)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| unavailable())?;
        if optional && matches!(response.status().as_u16(), 404 | 405 | 501) {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(upstream_error(response).await);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| unavailable())?;
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(
            serde_json::from_slice(&bytes).map_err(|_| unavailable())?,
        ))
    }

    pub async fn discover_models(&self, user: &Identity) -> Result<Vec<DiscoveredModel>, ApiError> {
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
                    supports_vision: bool_field("supports_vision"),
                    supports_function_calling: bool_field("supports_function_calling"),
                    supports_reasoning: bool_field("supports_reasoning"),
                    healthy: bool_field("healthy"),
                },
            });
        }
        Ok(models)
    }
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
/// The gateway's reason, when it gave one, follows Chat's own words, so a person can
/// tell a retired model from a bad request. It is cut short and kept to one line.
async fn upstream_error(response: Response) -> ApiError {
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
        _ => tokio::time::timeout(Duration::from_secs(5), response.bytes())
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|body| reason(&body)),
    };
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
fn reason(body: &[u8]) -> Option<String> {
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
#[cfg(test)]
mod tests {
    use super::*;

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
