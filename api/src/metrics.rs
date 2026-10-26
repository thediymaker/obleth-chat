use axum::{
    extract::{MatchedPath, Request, State},
    http::{header, HeaderMap},
    middleware::Next,
    response::Response,
};
use prometheus::{
    core::{Collector, Desc},
    proto::MetricFamily,
    Encoder, Gauge, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    Opts, Registry, TextEncoder,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct Metrics {
    pub registry: Registry,
    pub requests: IntCounterVec,
    pub latency: HistogramVec,
    pub streams: IntGauge,
    pub generations: IntCounterVec,
    pub tokens: IntCounterVec,
    pub missing_usage: IntCounter,
    pub first_token: Histogram,
    pub generation_duration: Histogram,
    /// The SHA-256 of the bearer token `/metrics` requires, when one is set.
    token: Option<Arc<[u8]>>,
}
impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new(
                "obleth_chat_http_requests_total",
                "HTTP requests by bounded method route and status",
            ),
            &["method", "route", "status"],
        )
        .unwrap();
        let latency = HistogramVec::new(
            HistogramOpts::new(
                "obleth_chat_http_header_duration_seconds",
                "Time to response headers; stream lifetime is measured separately",
            ),
            &["method", "route"],
        )
        .unwrap();
        let streams = IntGauge::new(
            "obleth_chat_active_streams",
            "In-flight chat generations on this process",
        )
        .unwrap();
        let generations = IntCounterVec::new(
            Opts::new(
                "obleth_chat_generations_total",
                "Finished generations by outcome",
            ),
            &["outcome"],
        )
        .unwrap();
        let tokens = IntCounterVec::new(
            Opts::new(
                "obleth_chat_tokens_total",
                "Provider-reported tokens by prompt or completion; no user or model labels",
            ),
            &["kind"],
        )
        .unwrap();
        let missing_usage = IntCounter::new(
            "obleth_chat_missing_usage_total",
            "Generations without a provider-reported total token count",
        )
        .unwrap();
        let first_token = Histogram::with_opts(
            HistogramOpts::new(
                "obleth_chat_first_token_seconds",
                "Time from accepted turn to first content token",
            )
            .buckets(vec![0.1, 0.25, 0.5, 1., 2., 5., 10., 30., 60., 120.]),
        )
        .unwrap();
        let generation_duration = Histogram::with_opts(
            HistogramOpts::new(
                "obleth_chat_generation_seconds",
                "Generation lifetime including persistence",
            )
            .buckets(vec![1., 5., 15., 30., 60., 120., 300., 600.]),
        )
        .unwrap();
        for c in [
            Box::new(requests.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(latency.clone()),
            Box::new(streams.clone()),
            Box::new(generations.clone()),
            Box::new(tokens.clone()),
            Box::new(missing_usage.clone()),
            Box::new(first_token.clone()),
            Box::new(generation_duration.clone()),
        ] {
            registry.register(c).unwrap();
        }
        Self {
            registry,
            requests,
            latency,
            streams,
            generations,
            tokens,
            missing_usage,
            first_token,
            generation_duration,
            token: None,
        }
    }
    /// Makes `/metrics` require `Authorization: Bearer <token>`.
    pub fn require_token(&mut self, token: &str) {
        self.token = Some(Sha256::digest(token.as_bytes()).to_vec().into());
    }
    /// Whether a request may read `/metrics`: always without a token set, otherwise
    /// only with that token as its one bearer credential. The comparison takes the
    /// same time whatever the token sent.
    pub fn allows(&self, headers: &HeaderMap) -> bool {
        let Some(expected) = &self.token else {
            return true;
        };
        let mut values = headers.get_all(header::AUTHORIZATION).iter();
        let (Some(value), None) = (values.next(), values.next()) else {
            return false;
        };
        let Some(sent) = value
            .to_str()
            .ok()
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim())
        else {
            return false;
        };
        bool::from(Sha256::digest(sent.as_bytes()).ct_eq(expected))
    }
    /// Exposes the management host's backup status ConfigMap, mounted at `dir`.
    pub fn register_backup_status(&self, dir: PathBuf) -> prometheus::Result<()> {
        self.registry.register(Box::new(BackupStatus {
            dir,
            last_success: Gauge::new(
                "obleth_chat_backup_last_success_timestamp_seconds",
                "Start time of the newest complete management-host backup",
            )?,
            bytes: Gauge::new(
                "obleth_chat_backup_bytes",
                "Size of the newest complete management-host backup",
            )?,
        }))
    }
    pub fn render(&self) -> String {
        let mut bytes = vec![];
        TextEncoder::new()
            .encode(&self.registry.gather(), &mut bytes)
            .unwrap();
        String::from_utf8(bytes).unwrap()
    }
}
/// Read at scrape time, so a ConfigMap update needs no restart. A missing or
/// invalid file omits that gauge rather than exporting NaN, which never
/// compares as stale; the alert treats absence as stale instead.
struct BackupStatus {
    dir: PathBuf,
    last_success: Gauge,
    bytes: Gauge,
}
impl Collector for BackupStatus {
    fn desc(&self) -> Vec<&Desc> {
        [&self.last_success, &self.bytes]
            .into_iter()
            .flat_map(|g| g.desc())
            .collect()
    }
    fn collect(&self) -> Vec<MetricFamily> {
        let mut families = vec![];
        for (gauge, file) in [(&self.last_success, "lastSuccess"), (&self.bytes, "bytes")] {
            if let Some(value) = read_status(&self.dir.join(file)) {
                gauge.set(value);
                families.extend(gauge.collect());
            }
        }
        families
    }
}
/// A non-negative decimal integer of at most 32 bytes including whitespace.
pub fn read_status(path: &Path) -> Option<f64> {
    let mut text = String::new();
    File::open(path)
        .ok()?
        .take(33)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() > 32 {
        return None;
    }
    text.trim().parse::<u64>().ok().map(|n| n as f64)
}
impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}
pub async fn record(State(metrics): State<Metrics>, request: Request, next: Next) -> Response {
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "static_or_unmatched".into());
    let method = match request.method().as_str() {
        "GET" => "GET",
        "HEAD" => "HEAD",
        "POST" => "POST",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        _ => "OTHER",
    };
    let started = Instant::now();
    let response = next.run(request).await;
    metrics
        .requests
        .with_label_values(&[method, &path, response.status().as_str()])
        .inc();
    metrics
        .latency
        .with_label_values(&[method, &path])
        .observe(started.elapsed().as_secs_f64());
    response
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn backup_status_is_read_at_scrape_time() {
        let dir = tempfile::tempdir().unwrap();
        let metrics = Metrics::new();
        assert!(!metrics.render().contains("obleth_chat_backup"));
        metrics
            .register_backup_status(dir.path().join("missing"))
            .unwrap();
        assert!(!metrics.render().contains("obleth_chat_backup"));
        let metrics = Metrics::new();
        metrics.register_backup_status(dir.path().into()).unwrap();
        assert!(!metrics.render().contains("obleth_chat_backup"));
        fs::write(dir.path().join("lastSuccess"), "1760000000\n").unwrap();
        fs::write(dir.path().join("bytes"), "52428800").unwrap();
        let text = metrics.render();
        assert!(text.contains("\nobleth_chat_backup_last_success_timestamp_seconds 1760000000\n"));
        assert!(text.contains("\nobleth_chat_backup_bytes 52428800\n"));
        assert!(text.contains("# TYPE obleth_chat_backup_bytes gauge"));
        fs::write(dir.path().join("lastSuccess"), "1760086400").unwrap();
        fs::write(dir.path().join("bytes"), "not a number").unwrap();
        let text = metrics.render();
        assert!(text.contains("\nobleth_chat_backup_last_success_timestamp_seconds 1760086400\n"));
        assert!(!text.contains("obleth_chat_backup_bytes"));
        assert!(metrics.register_backup_status(dir.path().into()).is_err());
    }

    #[test]
    fn invalid_status_values_are_omitted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("value");
        for (text, expected) in [
            (" 42 \n", Some(42.)),
            ("0", Some(0.)),
            ("", None),
            ("-1", None),
            ("1.5", None),
            ("NaN", None),
            ("inf", None),
            ("18446744073709551616", None),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(read_status(&path), expected, "{text:?}");
        }
        fs::write(&path, format!("{}1", " ".repeat(32))).unwrap();
        assert_eq!(read_status(&path), None);
        assert_eq!(read_status(dir.path()), None);
        assert_eq!(read_status(&dir.path().join("absent")), None);
    }
}
