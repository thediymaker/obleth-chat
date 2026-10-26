//! Minimal Kubernetes REST client for one namespace: create, read, list and delete
//! objects, and exec over the v5 WebSocket channel protocol. The service account
//! token is re-read on every call because projected tokens rotate.
use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Method, StatusCode};
use rustls::pki_types::{pem::PemObject, CertificateDer};
use serde_json::Value;
use std::{env, path::PathBuf, sync::Arc, time::Duration};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
    Connector,
};
use url::Url;

const SERVICE_ACCOUNT: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
/// Bytes kept per output stream; the newest bytes win.
pub const OUTPUT_LIMIT: usize = 256 * 1024;

pub type ExecSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Clone)]
pub struct Kube {
    http: reqwest::Client,
    tls: Arc<rustls::ClientConfig>,
    base: Url,
    token_file: PathBuf,
    namespace: String,
}

#[derive(Debug, Default)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// None when the process ended without an exit status (for example a timeout).
    pub exit_code: Option<i32>,
    pub truncated: bool,
}

impl Kube {
    /// In-cluster configuration, overridable for local development with
    /// OBLETH_CHAT_KUBE_API, OBLETH_CHAT_KUBE_TOKEN_FILE and OBLETH_CHAT_KUBE_CA_FILE.
    pub fn from_env(namespace: String) -> Result<Self> {
        let var = |key: &str| env::var(key).ok().filter(|v| !v.is_empty());
        let base = match var("OBLETH_CHAT_KUBE_API") {
            Some(url) => url,
            None => format!(
                "https://{}:{}",
                var("KUBERNETES_SERVICE_HOST")
                    .context("Kubernetes API address is not configured")?,
                var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|| "443".into())
            ),
        };
        let base = Url::parse(&base).context("Invalid Kubernetes API URL")?;
        if base.scheme() != "https" {
            bail!("The Kubernetes API URL must use https");
        }
        let token_file = var("OBLETH_CHAT_KUBE_TOKEN_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("{SERVICE_ACCOUNT}/token")));
        let ca_file = var("OBLETH_CHAT_KUBE_CA_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("{SERVICE_ACCOUNT}/ca.crt")));
        let mut roots = rustls::RootCertStore::empty();
        for cert in CertificateDer::pem_file_iter(&ca_file)
            .with_context(|| format!("Cannot read Kubernetes CA {}", ca_file.display()))?
        {
            roots.add(cert.context("Invalid Kubernetes CA certificate")?)?;
        }
        let tls = Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth(),
        );
        let http = reqwest::Client::builder()
            .use_preconfigured_tls((*tls).clone())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            tls,
            base,
            token_file,
            namespace,
        })
    }

    async fn token(&self) -> Result<String> {
        Ok(tokio::fs::read_to_string(&self.token_file)
            .await
            .context("Cannot read the Kubernetes service account token")?
            .trim()
            .to_owned())
    }

    fn url(&self, kind: &str, name: Option<&str>) -> Result<Url> {
        let path = format!("api/v1/namespaces/{}/{kind}", self.namespace);
        let path = match name {
            Some(name) => format!("{path}/{name}"),
            None => path,
        };
        Ok(self.base.join(&path)?)
    }

    async fn call(
        &self,
        method: Method,
        url: Url,
        body: Option<&Value>,
    ) -> Result<(StatusCode, Value)> {
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(self.token().await?);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .context("Kubernetes API request failed")?;
        let status = response.status();
        let value = response.json::<Value>().await.unwrap_or(Value::Null);
        Ok((status, value))
    }

    /// Creates an object; an existing object with the same name is not an error.
    pub async fn create(&self, kind: &str, object: &Value) -> Result<()> {
        let (status, value) = self
            .call(Method::POST, self.url(kind, None)?, Some(object))
            .await?;
        if status.is_success() || status == StatusCode::CONFLICT {
            return Ok(());
        }
        bail!("Kubernetes refused to create {kind}: {}", reason(&value))
    }

    pub async fn get(&self, kind: &str, name: &str) -> Result<Option<Value>> {
        let (status, value) = self
            .call(Method::GET, self.url(kind, Some(name))?, None)
            .await?;
        match status {
            s if s.is_success() => Ok(Some(value)),
            StatusCode::NOT_FOUND => Ok(None),
            _ => bail!("Kubernetes could not read {kind}: {}", reason(&value)),
        }
    }

    /// Deletes an object; a missing object is not an error.
    pub async fn delete(&self, kind: &str, name: &str) -> Result<()> {
        let (status, value) = self
            .call(Method::DELETE, self.url(kind, Some(name))?, None)
            .await?;
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        bail!("Kubernetes could not delete {kind}: {}", reason(&value))
    }

    /// Whether this service account may `verb` the namespaced `resource` (such as
    /// `pods` or `pods/exec`), from a SelfSubjectAccessReview.
    pub async fn allowed(&self, verb: &str, resource: &str) -> Result<bool> {
        let (resource, subresource) = resource.split_once('/').unwrap_or((resource, ""));
        let review = serde_json::json!({
            "apiVersion": "authorization.k8s.io/v1",
            "kind": "SelfSubjectAccessReview",
            "spec": {"resourceAttributes": {"namespace": self.namespace, "verb": verb, "resource": resource, "subresource": subresource}},
        });
        let url = self
            .base
            .join("apis/authorization.k8s.io/v1/selfsubjectaccessreviews")?;
        let (status, value) = self.call(Method::POST, url, Some(&review)).await?;
        if !status.is_success() {
            bail!("Kubernetes could not check access: {}", reason(&value));
        }
        Ok(value.pointer("/status/allowed").and_then(Value::as_bool) == Some(true))
    }

    pub async fn list(&self, kind: &str, selector: &str) -> Result<Vec<Value>> {
        let mut url = self.url(kind, None)?;
        url.query_pairs_mut().append_pair("labelSelector", selector);
        let (status, value) = self.call(Method::GET, url, None).await?;
        if !status.is_success() {
            bail!("Kubernetes could not list {kind}: {}", reason(&value));
        }
        Ok(value
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// Runs `argv` in the pod's `sandbox` container. Output beyond OUTPUT_LIMIT keeps
    /// the newest bytes. The caller bounds how long the command may run.
    pub async fn exec(&self, pod: &str, argv: &[&str], stdin: Option<&[u8]>) -> Result<ExecOutput> {
        self.exec_limited(pod, argv, stdin, OUTPUT_LIMIT).await
    }

    /// Like `exec`, keeping up to `limit` bytes per stream.
    pub async fn exec_limited(
        &self,
        pod: &str,
        argv: &[&str],
        stdin: Option<&[u8]>,
        limit: usize,
    ) -> Result<ExecOutput> {
        let mut socket = self.exec_socket(pod, argv, stdin.is_some(), false).await?;
        if let Some(data) = stdin {
            for chunk in data.chunks(64 * 1024) {
                let mut frame = Vec::with_capacity(chunk.len() + 1);
                frame.push(0);
                frame.extend_from_slice(chunk);
                socket.send(Message::Binary(frame.into())).await?;
            }
            // v5: close stdin so `cat > file` sees end of input.
            socket.send(Message::Binary(vec![255, 0].into())).await?;
        }
        let mut output = ExecOutput::default();
        while let Some(message) = socket.next().await {
            let data = match message? {
                Message::Binary(data) => data,
                Message::Close(_) => break,
                _ => continue,
            };
            let Some((&channel, payload)) = data.split_first() else {
                continue;
            };
            match channel {
                1 => append(&mut output.stdout, payload, &mut output.truncated, limit),
                2 => append(&mut output.stderr, payload, &mut output.truncated, limit),
                3 => output.exit_code = Some(exit_code(payload)),
                _ => {}
            }
        }
        Ok(output)
    }

    /// Opens an exec session in the pod's `sandbox` container over the v5 channel
    /// protocol: channel 0 is stdin, 1 stdout, 2 stderr, 3 the exit status and 4
    /// terminal resizes. With `tty`, output arrives on stdout only.
    pub async fn exec_socket(
        &self,
        pod: &str,
        argv: &[&str],
        stdin: bool,
        tty: bool,
    ) -> Result<ExecSocket> {
        let mut url = self.url("pods", Some(pod))?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Invalid Kubernetes API URL"))?
            .push("exec");
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("container", "sandbox");
            query.append_pair("stdout", "true");
            query.append_pair("stderr", if tty { "false" } else { "true" });
            if stdin {
                query.append_pair("stdin", "true");
            }
            if tty {
                query.append_pair("tty", "true");
            }
            for arg in argv {
                query.append_pair("command", arg);
            }
        }
        url.set_scheme("wss")
            .map_err(|_| anyhow::anyhow!("Invalid Kubernetes API URL"))?;
        let mut request = url.as_str().into_client_request()?;
        let headers = request.headers_mut();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.token().await?))?,
        );
        headers.insert(
            "sec-websocket-protocol",
            HeaderValue::from_static("v5.channel.k8s.io"),
        );
        let (socket, _) = connect_async_tls_with_config(
            request,
            None,
            false,
            Some(Connector::Rustls(self.tls.clone())),
        )
        .await
        .context("Could not open a Kubernetes exec session")?;
        Ok(socket)
    }
}

fn append(buffer: &mut Vec<u8>, data: &[u8], truncated: &mut bool, limit: usize) {
    buffer.extend_from_slice(data);
    if buffer.len() > limit {
        buffer.drain(..buffer.len() - limit);
        *truncated = true;
    }
}

/// The v5 status channel carries a metav1.Status: Success, or NonZeroExitCode with
/// the code in details.causes.
fn exit_code(payload: &[u8]) -> i32 {
    let Ok(status) = serde_json::from_slice::<Value>(payload) else {
        return -1;
    };
    if status.get("status").and_then(Value::as_str) == Some("Success") {
        return 0;
    }
    status
        .pointer("/details/causes")
        .and_then(Value::as_array)
        .and_then(|causes| {
            causes
                .iter()
                .find(|c| c.get("reason").and_then(Value::as_str) == Some("ExitCode"))
        })
        .and_then(|c| c.get("message").and_then(Value::as_str))
        .and_then(|code| code.parse().ok())
        .unwrap_or(-1)
}

/// The namespace Chat runs in, when it runs in a cluster with a service account
/// token mounted.
pub fn own_namespace() -> Option<String> {
    env::var("KUBERNETES_SERVICE_HOST").ok()?;
    std::fs::read_to_string(format!("{SERVICE_ACCOUNT}/namespace"))
        .ok()
        .map(|ns| ns.trim().to_owned())
        .filter(|ns| !ns.is_empty())
}

fn reason(value: &Value) -> String {
    value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unexpected response")
        .chars()
        .take(300)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_status_is_read_from_the_status_channel() {
        assert_eq!(exit_code(br#"{"status":"Success"}"#), 0);
        assert_eq!(
            exit_code(br#"{"status":"Failure","reason":"NonZeroExitCode","details":{"causes":[{"reason":"ExitCode","message":"2"}]}}"#),
            2
        );
        assert_eq!(exit_code(b"not json"), -1);
    }

    #[test]
    fn output_keeps_the_newest_bytes() {
        let mut buffer = Vec::new();
        let mut truncated = false;
        append(
            &mut buffer,
            &vec![b'a'; OUTPUT_LIMIT],
            &mut truncated,
            OUTPUT_LIMIT,
        );
        append(&mut buffer, b"end", &mut truncated, OUTPUT_LIMIT);
        assert!(truncated);
        assert_eq!(buffer.len(), OUTPUT_LIMIT);
        assert!(buffer.ends_with(b"end"));
    }
}
