//! A small S3 client for document files: put, get, delete, presigned downloads and
//! presigned uploads,
//! signed with AWS Signature Version 4. Works with S3, Cloudflare R2 and other
//! S3-compatible stores using path-style addresses (`<endpoint>/<bucket>/<key>`).

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::{env, time::Duration};
use url::Url;

const UNSIGNED: &str = "UNSIGNED-PAYLOAD";

#[derive(Clone)]
pub struct ObjectStore {
    client: reqwest::Client,
    endpoint: Url,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
}

impl ObjectStore {
    /// Reads OBLETH_CHAT_S3_ENDPOINT, _BUCKET, _ACCESS_KEY_ID, _SECRET_ACCESS_KEY and
    /// _REGION (default `auto`, which R2 expects). None when no endpoint is set.
    pub fn from_env() -> Result<Option<Self>> {
        let var = |key: &str| env::var(key).ok().filter(|v| !v.trim().is_empty());
        let Some(endpoint) = var("OBLETH_CHAT_S3_ENDPOINT") else {
            return Ok(None);
        };
        let endpoint = Url::parse(endpoint.trim()).context("Invalid OBLETH_CHAT_S3_ENDPOINT")?;
        if !matches!(endpoint.scheme(), "https" | "http") || endpoint.host_str().is_none() {
            bail!("OBLETH_CHAT_S3_ENDPOINT must be an http(s) URL");
        }
        let bucket = var("OBLETH_CHAT_S3_BUCKET").context("Set OBLETH_CHAT_S3_BUCKET")?;
        if !bucket
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        {
            bail!("OBLETH_CHAT_S3_BUCKET must be a plain bucket name");
        }
        Ok(Some(Self::new(
            endpoint,
            bucket,
            var("OBLETH_CHAT_S3_REGION").unwrap_or_else(|| "auto".into()),
            var("OBLETH_CHAT_S3_ACCESS_KEY_ID").context("Set OBLETH_CHAT_S3_ACCESS_KEY_ID")?,
            var("OBLETH_CHAT_S3_SECRET_ACCESS_KEY")
                .context("Set OBLETH_CHAT_S3_SECRET_ACCESS_KEY")?,
        )))
    }

    pub fn new(
        endpoint: Url,
        bucket: String,
        region: String,
        access_key: String,
        secret_key: String,
    ) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("http client"),
            endpoint,
            bucket,
            region,
            access_key,
            secret_key,
        }
    }

    fn object_url(&self, key: &str) -> Url {
        let mut url = self.endpoint.clone();
        let base = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{base}/{}/{}", self.bucket, encode(key, false)));
        url.set_query(None);
        url
    }

    pub async fn put(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<()> {
        let hash = hex(&Sha256::digest(&bytes));
        let url = self.object_url(key);
        let headers = self.sign("PUT", &url, &hash, Utc::now());
        let response = self
            .client
            .put(url)
            .headers(headers)
            .header("content-type", content_type)
            .body(bytes)
            .send()
            .await
            .context("object storage is unreachable")?;
        check(response).await.map(|_| ())
    }

    pub async fn get(&self, key: &str, limit: usize) -> Result<Vec<u8>> {
        let url = self.object_url(key);
        let headers = self.sign("GET", &url, UNSIGNED, Utc::now());
        let response = self
            .client
            .get(url)
            .headers(headers)
            .send()
            .await
            .context("object storage is unreachable")?;
        let response = check(response).await?;
        let bytes = response.bytes().await.context("reading the object")?;
        if bytes.len() > limit {
            bail!("the stored file is larger than expected");
        }
        Ok(bytes.to_vec())
    }

    /// Like `get`, but None when there is no such object.
    pub async fn get_optional(&self, key: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        let url = self.object_url(key);
        let headers = self.sign("GET", &url, UNSIGNED, Utc::now());
        let response = self
            .client
            .get(url)
            .headers(headers)
            .send()
            .await
            .context("object storage is unreachable")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = check(response).await?;
        let bytes = response.bytes().await.context("reading the object")?;
        if bytes.len() > limit {
            bail!("the stored file is larger than expected");
        }
        Ok(Some(bytes.to_vec()))
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        let url = self.object_url(key);
        let headers = self.sign("DELETE", &url, UNSIGNED, Utc::now());
        let response = self
            .client
            .delete(url)
            .headers(headers)
            .send()
            .await
            .context("object storage is unreachable")?;
        // Deleting something already gone is fine.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(response).await.map(|_| ())
    }

    /// A link that can upload one object, with no other credential, until it expires.
    pub fn presign_put(&self, key: &str, seconds: u64) -> Url {
        presign(
            "PUT",
            self.object_url(key),
            &self.region,
            &self.access_key,
            &self.secret_key,
            Utc::now(),
            seconds,
            &[],
        )
    }

    /// A short-lived link to the file under its name. PDFs and images open in the
    /// browser; anything else downloads, with text as plain text, whatever type the file
    /// was stored with (see `files::served_type`).
    pub fn presign_get(&self, key: &str, seconds: u64, name: &str, content_type: &str) -> Url {
        let (content_type, inline) = crate::files::served_type(content_type);
        let disposition = format!(
            "{}; filename=\"{}\"; filename*=UTF-8''{}",
            if inline { "inline" } else { "attachment" },
            name.chars()
                .map(
                    |c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                        c
                    } else {
                        '_'
                    }
                )
                .collect::<String>(),
            encode(name, true)
        );
        presign(
            "GET",
            self.object_url(key),
            &self.region,
            &self.access_key,
            &self.secret_key,
            Utc::now(),
            seconds,
            &[
                ("response-content-disposition", disposition.as_str()),
                ("response-content-type", content_type),
            ],
        )
    }

    fn sign(
        &self,
        method: &str,
        url: &Url,
        payload: &str,
        now: DateTime<Utc>,
    ) -> reqwest::header::HeaderMap {
        let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = &stamp[..8];
        let host = host(url);
        let canonical = format!(
            "{method}\n{}\n{}\nhost:{host}\nx-amz-content-sha256:{payload}\nx-amz-date:{stamp}\n\nhost;x-amz-content-sha256;x-amz-date\n{payload}",
            url.path(),
            canonical_query(url),
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let signature = signature(
            &self.secret_key,
            date,
            &self.region,
            &scope,
            &stamp,
            &canonical,
        );
        let mut headers = reqwest::header::HeaderMap::new();
        let mut set = |name: &'static str, value: String| {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(&value) {
                headers.insert(name, value);
            }
        };
        set("x-amz-date", stamp.clone());
        set("x-amz-content-sha256", payload.to_owned());
        set(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={signature}",
                self.access_key
            ),
        );
        headers
    }
}

async fn check(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    // S3 errors name a code such as NoSuchKey or AccessDenied; never echo credentials.
    let code = body
        .split("<Code>")
        .nth(1)
        .and_then(|rest| rest.split("</Code>").next())
        .unwrap_or("");
    bail!("object storage answered {status} {code}")
}

#[allow(clippy::too_many_arguments)]
fn presign(
    method: &str,
    mut url: Url,
    region: &str,
    access_key: &str,
    secret_key: &str,
    now: DateTime<Utc>,
    seconds: u64,
    extra: &[(&str, &str)],
) -> Url {
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = &stamp[..8];
    let scope = format!("{date}/{region}/s3/aws4_request");
    {
        let mut query = url.query_pairs_mut();
        query.clear();
        query.append_pair("X-Amz-Algorithm", "AWS4-HMAC-SHA256");
        query.append_pair("X-Amz-Credential", &format!("{access_key}/{scope}"));
        query.append_pair("X-Amz-Date", &stamp);
        query.append_pair("X-Amz-Expires", &seconds.to_string());
        query.append_pair("X-Amz-SignedHeaders", "host");
        for (key, value) in extra {
            query.append_pair(key, value);
        }
    }
    let canonical = format!(
        "{method}\n{}\n{}\nhost:{}\n\nhost\n{UNSIGNED}",
        url.path(),
        canonical_query(&url),
        host(&url)
    );
    let signature = signature(secret_key, date, region, &scope, &stamp, &canonical);
    // Rebuild the query in canonical encoding so the signed string and the URL agree.
    let query = format!("{}&X-Amz-Signature={signature}", canonical_query(&url));
    url.set_query(Some(&query));
    url
}

fn signature(
    secret: &str,
    date: &str,
    region: &str,
    scope: &str,
    stamp: &str,
    canonical: &str,
) -> String {
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let key = [date, region, "s3", "aws4_request"]
        .iter()
        .fold(format!("AWS4{secret}").into_bytes(), |key, part| {
            mac(&key, part.as_bytes())
        });
    hex(&mac(&key, to_sign.as_bytes()))
}

fn mac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn host(url: &Url) -> String {
    match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_owned(),
    }
}

/// Query parameters sorted and encoded the way SigV4 signs them.
fn canonical_query(url: &Url) -> String {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (encode(&k, true), encode(&v, true)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// RFC 3986 encoding: unreserved characters stay, everything else is %XX; slashes stay
/// in object keys unless `slash` is set.
fn encode(value: &str, slash: bool) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            b'/' if !slash => "/".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presigned_urls_match_the_aws_example() {
        // From the AWS documentation for query-string authentication.
        let url = presign(
            "GET",
            Url::parse("https://examplebucket.s3.amazonaws.com/test.txt").unwrap(),
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            DateTime::parse_from_rfc3339("2013-05-24T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            86400,
            &[],
        );
        assert!(
            url.as_str().ends_with(
                "X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
            ),
            "{url}"
        );
    }

    #[test]
    fn links_never_open_active_content_in_place() {
        let store = ObjectStore {
            client: reqwest::Client::new(),
            endpoint: Url::parse("https://storage.example.com").unwrap(),
            bucket: "bucket".into(),
            region: "auto".into(),
            access_key: "access".into(),
            secret_key: "secret".into(),
        };
        let link = |name: &str, stored: &str| {
            let url = store.presign_get("knowledge/a/b", 300, name, stored);
            let query: std::collections::HashMap<String, String> =
                url.query_pairs().into_owned().collect();
            (
                query["response-content-type"].clone(),
                query["response-content-disposition"].clone(),
            )
        };
        let (kind, disposition) = link("page.html", "text/html; charset=utf-8");
        assert_eq!(kind, "text/plain; charset=utf-8");
        assert!(disposition.starts_with("attachment; "), "{disposition}");
        let (kind, disposition) = link("notes.txt", "image/svg+xml; x=json");
        assert_eq!(kind, "text/plain; charset=utf-8");
        assert!(disposition.starts_with("attachment; "), "{disposition}");
        let (kind, disposition) = link("Lecture 7 \"ü\".pdf", "application/pdf");
        assert_eq!(kind, "application/pdf");
        assert_eq!(
            disposition,
            "inline; filename=\"Lecture 7 ___.pdf\"; filename*=UTF-8''Lecture%207%20%22%C3%BC%22.pdf"
        );
    }

    #[test]
    fn keys_are_encoded_as_paths() {
        assert_eq!(
            encode("knowledge/a b/ü.pdf", false),
            "knowledge/a%20b/%C3%BC.pdf"
        );
        assert_eq!(encode("a/b", true), "a%2Fb");
    }
}
