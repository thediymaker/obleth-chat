//! A Model Context Protocol client over the Streamable HTTP transport: JSON-RPC
//! requests are POSTed to one endpoint, which answers with JSON or a short SSE
//! stream. Connections people add can reach only public addresses: the resolver
//! drops private ones, so a hostname cannot be pointed at the cluster later.
use crate::gateway::Decoder;
use futures_util::StreamExt;
use reqwest::{
    dns::{Addrs, Name, Resolve, Resolving},
    header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE},
    StatusCode,
};
use serde_json::{json, Value};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use url::{Host, Url};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Largest response read from a server.
const MAX_RESPONSE: usize = 8 * 1024 * 1024;
const MAX_TOOLS: usize = 128;
const MAX_SCHEMA: usize = 32 * 1024;
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const SETUP_TIMEOUT: Duration = Duration::from_secs(20);

/// HTTP clients for connections: `public` for ones people add, `any` for ones an
/// administrator shares, which may name internal services.
#[derive(Clone)]
pub struct Clients {
    pub public: reqwest::Client,
    pub any: reqwest::Client,
}
impl Clients {
    pub fn new(private_allowed: bool) -> anyhow::Result<Self> {
        let any = client(true, None)?;
        let public = if private_allowed {
            any.clone()
        } else {
            client(false, None)?
        };
        Ok(Self { public, any })
    }
}

/// An HTTP client for one kind of connection: `private` lets it reach internal
/// addresses, and `ca` adds a cluster's own certificate authority (PEM).
pub fn client(private: bool, ca: Option<&str>) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("obleth-chat/", env!("CARGO_PKG_VERSION")));
    if !private {
        builder = builder.dns_resolver(Arc::new(PublicOnly));
    }
    if let Some(pem) = ca {
        for certificate in reqwest::Certificate::from_pem_bundle(pem.as_bytes())? {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder.build()?)
}

#[derive(Debug)]
struct Blocked;
impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the host resolves only to private addresses")
    }
}
impl std::error::Error for Blocked {}

struct PublicOnly;
impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let found: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| is_public(a.ip()))
                .collect();
            if found.is_empty() {
                return Err(Box::new(Blocked) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(found.into_iter()) as Addrs)
        })
    }
}

/// Globally routable unicast addresses only.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_v4(v4);
            }
            let s = v6.segments();
            // NAT64 embeds an IPv4 address in its last 32 bits.
            if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] {
                return public_v4(Ipv4Addr::new(
                    (s[6] >> 8) as u8,
                    s[6] as u8,
                    (s[7] >> 8) as u8,
                    s[7] as u8,
                ));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] == 0x2001 && s[1] == 0x0db8))
        }
    }
}
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 192 && b == 0 && ip.octets()[2] == 0))
}

/// A server URL people may save: https to a public host, or for administrators
/// any http(s) address.
pub fn check_url(raw: &str, private_allowed: bool) -> Result<Url, String> {
    let url = Url::parse(raw.trim()).map_err(|_| "Enter the server's full URL".to_string())?;
    let scheme_ok = url.scheme() == "https" || (private_allowed && url.scheme() == "http");
    if !scheme_ok {
        return Err(if private_allowed {
            "The URL must start with http:// or https://".into()
        } else {
            "The URL must start with https://".into()
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Put credentials in the token field, not the URL".into());
    }
    if url.fragment().is_some() {
        return Err("The URL cannot contain a # fragment".into());
    }
    let private = match url.host() {
        None => return Err("The URL needs a host name".into()),
        Some(Host::Ipv4(ip)) => !is_public(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => !is_public(IpAddr::V6(ip)),
        Some(Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
    };
    if private && !private_allowed {
        return Err("Connections can only reach public servers".into());
    }
    Ok(url)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}
impl Tool {
    pub fn to_json(&self) -> Value {
        json!({"name":self.name,"title":self.title,"description":self.description,"inputSchema":self.input_schema,"readOnly":self.read_only})
    }
    pub fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            name: value.get("name")?.as_str()?.to_owned(),
            title: value
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned),
            description: value
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            input_schema: value
                .get("inputSchema")
                .cloned()
                .unwrap_or(json!({"type":"object"})),
            read_only: value.get("readOnly").and_then(Value::as_bool) == Some(true),
        })
    }
}

/// One tool from a tools/list page, or None when it cannot be offered.
fn parse_tool(value: &Value) -> Option<Tool> {
    let name = value.get("name")?.as_str()?;
    if name.is_empty()
        || name.len() > 128
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c))
    {
        return None;
    }
    let annotations = value.get("annotations");
    let title = value
        .get("title")
        .or_else(|| annotations.and_then(|a| a.get("title")))
        .and_then(Value::as_str)
        .map(|t| t.chars().take(120).collect());
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(2000)
        .collect();
    let mut input_schema = value
        .get("inputSchema")
        .filter(|s| s.is_object())
        .cloned()
        .unwrap_or(json!({"type":"object"}));
    if input_schema.to_string().len() > MAX_SCHEMA {
        input_schema = json!({"type":"object"});
    }
    if input_schema.get("type").is_none() {
        input_schema["type"] = json!("object");
    }
    Some(Tool {
        name: name.to_owned(),
        title,
        description,
        input_schema,
        read_only: annotations
            .and_then(|a| a.get("readOnlyHint"))
            .and_then(Value::as_bool)
            == Some(true),
    })
}

/// A tools/call result as text for the model, and whether the tool succeeded.
pub fn result_text(result: &Value) -> (bool, String) {
    let ok = result.get("isError").and_then(Value::as_bool) != Some(true);
    let mut parts = vec![];
    for item in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("");
        match text("type") {
            "text" => parts.push(text("text").to_owned()),
            "image" | "audio" => {
                parts.push(format!("[{} omitted: {}]", text("type"), text("mimeType")))
            }
            "resource" => {
                let resource = item.get("resource").cloned().unwrap_or_default();
                match resource.get("text").and_then(Value::as_str) {
                    Some(body) => parts.push(body.to_owned()),
                    None => parts.push(format!(
                        "[Resource {}]",
                        resource.get("uri").and_then(Value::as_str).unwrap_or("")
                    )),
                }
            }
            "resource_link" => parts.push(format!("[Resource {} {}]", text("name"), text("uri"))),
            _ => {}
        }
    }
    if parts.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            parts.push(structured.to_string());
        }
    }
    let text = parts.join("\n");
    (
        ok,
        if text.is_empty() {
            "(no output)".into()
        } else {
            text
        },
    )
}

pub struct Session {
    http: reqwest::Client,
    url: Url,
    token: Option<String>,
    session: Option<String>,
    version: String,
    next_id: u64,
    pub server_name: Option<String>,
}

impl Session {
    /// Connects and completes the initialize handshake.
    pub async fn open(
        http: reqwest::Client,
        url: Url,
        token: Option<String>,
    ) -> Result<Self, String> {
        let mut session = Self {
            http,
            url,
            token,
            session: None,
            version: PROTOCOL_VERSION.into(),
            next_id: 1,
            server_name: None,
        };
        let result = session
            .request(
                "initialize",
                json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"obleth-chat","version":env!("CARGO_PKG_VERSION")}}),
                SETUP_TIMEOUT,
            )
            .await?;
        if let Some(version) = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .filter(|v| v.len() <= 32)
        {
            session.version = version.to_owned();
        }
        if result.pointer("/capabilities/tools").is_none() {
            return Err("This server does not offer any tools".into());
        }
        session.server_name = result
            .pointer("/serverInfo/title")
            .or_else(|| result.pointer("/serverInfo/name"))
            .and_then(Value::as_str)
            .map(|n| n.chars().take(200).collect());
        session.notify("notifications/initialized").await?;
        Ok(session)
    }

    pub async fn list_tools(&mut self) -> Result<Vec<Tool>, String> {
        let mut tools = vec![];
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let params = match &cursor {
                Some(c) => json!({"cursor":c}),
                None => json!({}),
            };
            let page = self.request("tools/list", params, SETUP_TIMEOUT).await?;
            for tool in page
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(parse_tool)
            {
                if tools.len() < MAX_TOOLS && !tools.iter().any(|t: &Tool| t.name == tool.name) {
                    tools.push(tool);
                }
            }
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty() && c.len() <= 1024)
                .map(str::to_owned);
            if cursor.is_none() || tools.len() >= MAX_TOOLS {
                break;
            }
        }
        Ok(tools)
    }

    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        self.request(
            "tools/call",
            json!({"name":name,"arguments":arguments}),
            CALL_TIMEOUT,
        )
        .await
    }

    /// Ends the server-side session, if the server keeps one.
    pub async fn close(self) {
        let Some(id) = &self.session else { return };
        let mut request = self
            .http
            .delete(self.url.clone())
            .timeout(Duration::from_secs(3))
            .header("mcp-session-id", id)
            .header("mcp-protocol-version", &self.version);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let _ = request.send().await;
    }

    fn post(&self, body: &Value, timeout: Duration) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .post(self.url.clone())
            .timeout(timeout)
            .header(ACCEPT, "application/json, text/event-stream")
            .json(body);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(id) = &self.session {
            request = request.header("mcp-session-id", id);
        }
        if self.next_id > 1 {
            request = request.header("mcp-protocol-version", &self.version);
        }
        request
    }

    async fn notify(&mut self, method: &str) -> Result<(), String> {
        let body = json!({"jsonrpc":"2.0","method":method});
        let response = self
            .post(&body, SETUP_TIMEOUT)
            .send()
            .await
            .map_err(|e| describe(&e, &self.url))?;
        check_status(&response)?;
        Ok(())
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id;
        let body = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let request = self.post(&body, timeout);
        self.next_id += 1;
        let response = request.send().await.map_err(|e| describe(&e, &self.url))?;
        check_status(&response)?;
        if method == "initialize" {
            self.session = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .filter(|v| {
                    !v.is_empty() && v.len() <= 256 && v.bytes().all(|b| (0x21..=0x7e).contains(&b))
                })
                .map(str::to_owned);
        }
        let streaming = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let message = if streaming {
            read_stream(response, id, &self.url).await?
        } else {
            let bytes = read_limited(response, &self.url).await?;
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| "The server's answer was not MCP (expected JSON-RPC)".to_string())?;
            match value {
                Value::Array(items) => items
                    .into_iter()
                    .find(|m| m.get("id") == Some(&json!(id)))
                    .ok_or("The server's answer did not match the request")?,
                message => message,
            }
        };
        if let Some(error) = message.get("error") {
            let text = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(format!(
                "The server returned an error: {}",
                text.chars().take(300).collect::<String>()
            ));
        }
        message
            .get("result")
            .cloned()
            .ok_or_else(|| "The server's answer had no result".into())
    }
}

fn check_status(response: &reqwest::Response) -> Result<(), String> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            let oauth = response
                .headers()
                .get(WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("resource_metadata"));
            if oauth {
                format!("The server wants an OAuth sign-in (HTTP {}), which Chat does not support yet. Use an access token instead.", status.as_u16())
            } else {
                format!("The server refused the token (HTTP {})", status.as_u16())
            }
        }
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => format!(
            "No MCP endpoint at this URL (HTTP {}). Check the path. Servers that only offer the older SSE transport are not supported.",
            status.as_u16()
        ),
        s if s.is_redirection() => format!(
            "The server redirected the request (HTTP {}). Enter the final URL.",
            s.as_u16()
        ),
        s => format!("The server answered HTTP {}", s.as_u16()),
    })
}

fn describe(error: &reqwest::Error, url: &Url) -> String {
    let host = url.host_str().unwrap_or("the server");
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        if cause.downcast_ref::<Blocked>().is_some() {
            return format!("{host} resolves to a private address, which connections cannot reach");
        }
        source = cause.source();
    }
    if error.is_timeout() {
        format!("{host} did not answer in time")
    } else if error.is_connect() {
        format!("Could not connect to {host}")
    } else {
        format!("The request to {host} failed")
    }
}

async fn read_limited(response: reqwest::Response, url: &Url) -> Result<Vec<u8>, String> {
    let mut body = vec![];
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| describe(&e, url))?;
        if body.len() + chunk.len() > MAX_RESPONSE {
            return Err("The server's answer was too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads SSE events until the response to `id`; notifications are skipped.
async fn read_stream(response: reqwest::Response, id: u64, url: &Url) -> Result<Value, String> {
    let mut decoder = Decoder::default();
    let mut total = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| describe(&e, url))?;
        total += chunk.len();
        if total > MAX_RESPONSE {
            return Err("The server's answer was too large".into());
        }
        let events = decoder
            .push(&chunk)
            .map_err(|_| "The server sent an invalid event stream".to_string())?;
        for event in events {
            let Ok(message) = serde_json::from_str::<Value>(&event) else {
                continue;
            };
            if message.get("id") == Some(&json!(id))
                && (message.get("result").is_some() || message.get("error").is_some())
            {
                return Ok(message);
            }
        }
    }
    Err("The server closed the stream without answering".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_addresses_pass() {
        for ip in [
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "198.18.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip} should be private");
        }
        for ip in [
            "104.16.160.175",
            "8.8.8.8",
            "2606:4700::6810:a0af",
            "64:ff9b::808:808",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip} should be public");
        }
    }

    #[test]
    fn saved_urls_are_checked() {
        assert!(check_url("https://mcp.example.com/mcp", false).is_ok());
        assert!(check_url("http://mcp.example.com/mcp", false).is_err());
        assert!(check_url("https://10.0.0.5/mcp", false).is_err());
        assert!(check_url("https://[::1]/mcp", false).is_err());
        assert!(check_url("https://localhost/mcp", false).is_err());
        assert!(check_url("https://user:pw@mcp.example.com/", false).is_err());
        assert!(check_url("ftp://mcp.example.com/", true).is_err());
        assert!(check_url("http://mcp.mcp-test.svc:8080/mcp", true).is_ok());
        assert!(check_url("http://127.0.0.1:9/mcp", true).is_ok());
    }

    #[test]
    fn tools_are_parsed_defensively() {
        let tool = parse_tool(&json!({"name":"get_job","description":"Show a job","inputSchema":{"properties":{"id":{"type":"string"}}},"annotations":{"readOnlyHint":true}})).unwrap();
        assert!(tool.read_only);
        assert_eq!(tool.input_schema["type"], "object");
        assert!(
            !parse_tool(&json!({"name":"submit","inputSchema":{"type":"object"}}))
                .unwrap()
                .read_only
        );
        assert!(parse_tool(&json!({"name":"bad name"})).is_none());
        assert!(parse_tool(&json!({"description":"no name"})).is_none());
        assert_eq!(Tool::from_json(&tool.to_json()), Some(tool));
    }

    #[test]
    fn results_become_text() {
        let (ok, text) = result_text(
            &json!({"content":[{"type":"text","text":"3 jobs"},{"type":"image","mimeType":"image/png","data":"…"}]}),
        );
        assert!(ok);
        assert_eq!(text, "3 jobs\n[image omitted: image/png]");
        let (ok, text) =
            result_text(&json!({"isError":true,"content":[{"type":"text","text":"no such job"}]}));
        assert!(!ok);
        assert_eq!(text, "no such job");
        assert_eq!(
            result_text(&json!({"structuredContent":{"n":1}})).1,
            r#"{"n":1}"#
        );
        assert_eq!(result_text(&json!({})).1, "(no output)");
    }
}
