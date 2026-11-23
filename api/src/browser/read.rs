//! Reading a page to keep it, for a notebook. Only the person's own browser reads it:
//! a web page is taken from the page as it renders there, as Markdown (so pages built
//! with JavaScript, and pages behind the person's sign-in, read as they see them), and
//! a PDF is taken from the browser's own answer before its viewer opens it. Chat never
//! fetches the address itself. Reading opens and submits nothing on the page, so no
//! site permission is asked for or used.
use super::{close_scratch, tools::bot_check, tools::wait_ready, Browsers};
use crate::browser::cdp::Cdp;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Longest a page may take to answer and load.
const LOAD_TIME: Duration = Duration::from_secs(45);
/// Bytes of a PDF read from the browser at a time.
const CHUNK: usize = 1 << 20;
/// Letters and digits a page needs to count as having readable text.
const LEAST_TEXT: usize = 80;
/// A page with a password field and less text than this is a sign-in page.
const SIGN_IN_TEXT: usize = 1500;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Characters of a page's text.
    pub text_chars: usize,
    /// Bytes of a PDF.
    pub pdf_bytes: usize,
    /// Whether every address the read passes through must be a public web site's, by
    /// its text and by what its name resolves to: the one asked for, each one a
    /// redirect leads to, each page a frame of it loads before it is asked for, and
    /// the one it ends at. Its scripts' other requests are left to the sandbox
    /// namespace's network rules.
    pub public_only: bool,
}

/// What a read says when it was led somewhere that is not a public web site.
const NOT_PUBLIC: &str = "The page led to an address that is not a public web site.";

async fn allowed(limits: Limits, url: &str) -> bool {
    use crate::notebook::web::{address, resolves_public};
    !limits.public_only || (address(url).is_ok() && resolves_public(url).await)
}

#[derive(Debug, PartialEq)]
pub enum Read {
    /// A web page's readable text as Markdown, with its title.
    Page {
        url: String,
        title: String,
        text: String,
    },
    /// A PDF, with a file name for it.
    Pdf {
        url: String,
        name: String,
        bytes: Vec<u8>,
    },
}

impl Browsers {
    /// Reads `url` in a tab of its own in the person's browser, closed afterwards.
    /// Errors are sentences for the person.
    pub async fn read_address(
        &self,
        owner: Uuid,
        url: &str,
        limits: Limits,
    ) -> Result<Read, String> {
        let (cdp, session, target) = self.scratch(owner).await?;
        let read = read_in(&cdp, &session, &target, url, limits).await;
        close_scratch(&cdp, &target).await;
        read
    }

    /// Reads the page a chat's tab shows now, as it is; a PDF it shows is read again
    /// from its address in a tab of its own. Starts nothing: the browser must be up.
    pub async fn read_tab(
        &self,
        owner: Uuid,
        conversation: Uuid,
        limits: Limits,
    ) -> Result<Read, String> {
        let running = self
            .get(owner)
            .await
            .map_err(|e| e.1)?
            .is_some_and(|b| b.status == "running");
        let tab = self.get_tab(owner, conversation).await.map_err(|e| e.1)?;
        if !running || tab.as_ref().and_then(|t| t.target_id.as_ref()).is_none() {
            return Err("The browser is not open in this chat. Open the page in it first.".into());
        }
        let page = self.open(owner, conversation).await?;
        // A PDF shows in the browser's viewer, whose page does not read; it is known
        // by its frame's type.
        let frame = page
            .cdp
            .call("Page.getResourceTree", json!({}), Some(&page.session))
            .await
            .map_err(|e| format!("The page could not be read: {e:#}"))?;
        let frame = &frame["frameTree"]["frame"];
        if frame["mimeType"] == "application/pdf" {
            let url = frame["url"].as_str().unwrap_or("").to_owned();
            drop(page);
            return self.read_address(owner, &url, limits).await;
        }
        let value = page
            .cdp
            .eval(&page.session, &readable(limits.text_chars))
            .await
            .map_err(|e| format!("The page could not be read: {e:#}"))?;
        let url = value["url"].as_str().unwrap_or("").to_owned();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err("There is no web page open in this chat’s browser tab.".into());
        }
        if value["pdf"] == true {
            drop(page);
            return self.read_address(owner, &url, limits).await;
        }
        judge(&value, limits)
    }
}

/// Opens `url` in the tab and reads it. The page's own answer is held at its headers
/// (the Fetch domain, for documents only), so a PDF is taken as a file and an error
/// status is reported, before anything renders.
async fn read_in(
    cdp: &Cdp,
    session: &str,
    target: &str,
    url: &str,
    limits: Limits,
) -> Result<Read, String> {
    if !allowed(limits, url).await {
        return Err(NOT_PUBLIC.into());
    }
    let failed = |e: anyhow::Error| format!("The browser failed: {e:#}");
    let events = cdp.events();
    cdp.call(
        "Fetch.enable",
        json!({"patterns":[
            {"urlPattern":"*","resourceType":"Document","requestStage":"Request"},
            {"urlPattern":"*","resourceType":"Document","requestStage":"Response"},
        ]}),
        Some(session),
    )
    .await
    .map_err(failed)?;
    let loaded = load(cdp, session, target, url, limits, events).await;
    let _ = cdp.call("Fetch.disable", json!({}), Some(session)).await;
    if let Some(pdf) = loaded? {
        return Ok(pdf);
    }
    wait_ready(cdp, session).await;
    let read = || async {
        cdp.eval(session, &readable(limits.text_chars))
            .await
            .map_err(|e| format!("The page could not be read: {e:#}"))
    };
    let mut value = read().await?;
    // A page that builds itself after loading gets a moment more.
    if letters(value["text"].as_str().unwrap_or("")) < LEAST_TEXT * 4 && value["pdf"] != true {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        value = read().await?;
    }
    if value["pdf"] == true {
        return Err("The browser showed a PDF it would not hand over. Try again.".into());
    }
    // Where the page ended up, after anything its own script did.
    if !allowed(limits, value["url"].as_str().unwrap_or("")).await {
        return Err(NOT_PUBLIC.into());
    }
    judge(&value, limits)
}

/// Navigates to `url` and waits for its answer: a PDF's bytes, or None once a page
/// is loading.
async fn load(
    cdp: &Cdp,
    session: &str,
    target: &str,
    url: &str,
    limits: Limits,
    mut events: tokio::sync::mpsc::UnboundedReceiver<Value>,
) -> Result<Option<Read>, String> {
    let failed = |e: anyhow::Error| format!("The browser failed: {e:#}");
    let navigate = cdp.call("Page.navigate", json!({"url":url}), Some(session));
    tokio::pin!(navigate);
    let deadline = tokio::time::sleep(LOAD_TIME);
    tokio::pin!(deadline);
    let mut answered = false;
    loop {
        tokio::select! {
            result = &mut navigate => {
                let result = result.map_err(failed)?;
                if let Some(error) = result.get("errorText").and_then(Value::as_str) {
                    return Err(load_error(error));
                }
                // The browser answered without the page reaching us, such as a jump
                // within the page; read what it shows.
                break;
            }
            event = events.recv() => {
                let Some(event) = event else {
                    return Err("The browser connection closed while the page loaded.".into());
                };
                if event["method"] != "Fetch.requestPaused" || event["sessionId"] != session {
                    continue;
                }
                let p = &event["params"];
                let id = p["requestId"].as_str().unwrap_or("").to_owned();
                // A page asked for and not yet sent, in any frame and at any point of
                // the load: one off the public web is never sent.
                if p.get("responseStatusCode").is_none() && p.get("responseErrorReason").is_none() {
                    let asked = p.pointer("/request/url").and_then(Value::as_str).unwrap_or("");
                    if allowed(limits, asked).await {
                        let _ = cdp
                            .call("Fetch.continueRequest", json!({"requestId":id}), Some(session))
                            .await;
                    } else {
                        abort(cdp, session, &id).await;
                        if p["frameId"] == target {
                            return Err(NOT_PUBLIC.into());
                        }
                    }
                    continue;
                }
                let status = p["responseStatusCode"].as_u64().unwrap_or(0);
                let headers = headers(&p["responseHeaders"]);
                let main = p["frameId"] == target && !answered;
                let at = p.pointer("/request/url").and_then(Value::as_str).unwrap_or(url);
                let header = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
                let redirect = (300..400).contains(&status) && header("location").is_some();
                // Neither the page's own address nor one a redirect points at may leave
                // the public web; a redirect is stopped before the browser follows it.
                if main {
                    let next = header("location")
                        .filter(|_| redirect)
                        .and_then(|to| url::Url::parse(at).ok()?.join(to).ok());
                    let next_allowed = match &next {
                        Some(to) => allowed(limits, to.as_str()).await,
                        None => true,
                    };
                    if !allowed(limits, at).await || !next_allowed {
                        abort(cdp, session, &id).await;
                        return Err(NOT_PUBLIC.into());
                    }
                }
                if !main || redirect || p.get("responseErrorReason").is_some() || status == 0 {
                    resume(cdp, session, &id).await;
                    continue;
                }
                if status >= 400 {
                    abort(cdp, session, &id).await;
                    return Err(status_error(status));
                }
                if is_pdf(header("content-type").unwrap_or(""), at) {
                    let declared = header("content-length").and_then(|n| n.trim().parse::<usize>().ok());
                    if declared.is_some_and(|n| n > limits.pdf_bytes) {
                        abort(cdp, session, &id).await;
                        return Err(pdf_too_large(limits));
                    }
                    let bytes = take_body(cdp, session, &id, limits).await;
                    abort(cdp, session, &id).await;
                    return Ok(Some(Read::Pdf { url: at.to_owned(), name: pdf_name(at, header("content-disposition")), bytes: bytes? }));
                }
                resume(cdp, session, &id).await;
                answered = true;
            }
            _ = &mut deadline => return Err("The page did not load within 45 seconds.".into()),
        }
    }
    Ok(None)
}

/// A response's headers, names in lower case.
fn headers(list: &Value) -> Vec<(String, String)> {
    list.as_array()
        .map(|all| {
            all.iter()
                .map(|h| {
                    (
                        h["name"].as_str().unwrap_or("").to_ascii_lowercase(),
                        h["value"].as_str().unwrap_or("").to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn resume(cdp: &Cdp, session: &str, id: &str) {
    if cdp
        .call(
            "Fetch.continueResponse",
            json!({"requestId":id}),
            Some(session),
        )
        .await
        .is_err()
    {
        let _ = cdp
            .call(
                "Fetch.continueRequest",
                json!({"requestId":id}),
                Some(session),
            )
            .await;
    }
}

async fn abort(cdp: &Cdp, session: &str, id: &str) {
    let _ = cdp
        .call(
            "Fetch.failRequest",
            json!({"requestId":id,"errorReason":"Aborted"}),
            Some(session),
        )
        .await;
}

/// The held answer's body, read in pieces up to the PDF limit.
async fn take_body(cdp: &Cdp, session: &str, id: &str, limits: Limits) -> Result<Vec<u8>, String> {
    let failed = |e: anyhow::Error| format!("The browser could not hand over the PDF: {e:#}");
    let stream = cdp
        .call(
            "Fetch.takeResponseBodyAsStream",
            json!({"requestId":id}),
            Some(session),
        )
        .await
        .map_err(failed)?;
    let handle = stream["stream"]
        .as_str()
        .ok_or("The browser could not hand over the PDF.")?
        .to_owned();
    let mut bytes: Vec<u8> = Vec::new();
    let result = loop {
        let chunk = match cdp
            .call(
                "IO.read",
                json!({"handle":handle,"size":CHUNK}),
                Some(session),
            )
            .await
        {
            Ok(chunk) => chunk,
            Err(e) => break Err(failed(e)),
        };
        let data = chunk["data"].as_str().unwrap_or("");
        if chunk["base64Encoded"] == true {
            match STANDARD.decode(data) {
                Ok(part) => bytes.extend(part),
                Err(_) => break Err("The browser handed over an unreadable PDF.".into()),
            }
        } else {
            bytes.extend(data.as_bytes());
        }
        if bytes.len() > limits.pdf_bytes {
            break Err(pdf_too_large(limits));
        }
        if chunk["eof"] == true || data.is_empty() {
            break Ok(());
        }
    };
    let _ = cdp
        .call("IO.close", json!({"handle":handle}), Some(session))
        .await;
    result?;
    if !bytes
        .get(..1024)
        .unwrap_or(&bytes)
        .windows(5)
        .any(|w| w == b"%PDF-")
    {
        return Err("The address gave a file that is neither a web page nor a PDF.".into());
    }
    Ok(bytes)
}

/// Whether an answer is a PDF: by its type, or a generic file type at a .pdf address.
pub(crate) fn is_pdf(content_type: &str, url: &str) -> bool {
    let kind = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match kind.as_str() {
        "application/pdf" | "application/x-pdf" => true,
        "" | "application/octet-stream" | "binary/octet-stream" | "application/download" => {
            url::Url::parse(url).is_ok_and(|u| u.path().to_ascii_lowercase().ends_with(".pdf"))
        }
        _ => false,
    }
}

/// A file name for a PDF: the one the site gives, else the address's last part.
pub(crate) fn pdf_name(url: &str, disposition: Option<&str>) -> String {
    let given = disposition.and_then(|d| {
        d.split(';').find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key.trim().eq_ignore_ascii_case("filename"))
                .then(|| value.trim().trim_matches('"').to_owned())
        })
    });
    let from_url = || {
        url::Url::parse(url).ok().and_then(|u| {
            let last = u.path_segments()?.rev().find(|s| !s.is_empty())?.to_owned();
            Some(
                url::form_urlencoded::parse(format!("x={last}").as_bytes())
                    .next()
                    .map_or(last, |(_, v)| v.into_owned()),
            )
        })
    };
    let name: String = given
        .or_else(from_url)
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\'))
        .take(180)
        .collect();
    let name = name.trim().to_owned();
    let name = if name.is_empty() || name.starts_with('.') {
        url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| "document".into())
    } else {
        name
    };
    if name.to_ascii_lowercase().ends_with(".pdf") {
        name
    } else {
        format!("{name}.pdf")
    }
}

fn pdf_too_large(limits: Limits) -> String {
    format!(
        "The PDF is too large: a notebook source can be at most {}.",
        crate::attachments::size_label(limits.pdf_bytes)
    )
}

fn load_error(error: &str) -> String {
    match error {
        "net::ERR_ABORTED" => {
            "The address is a file download, not a page, and only web pages and PDFs can be added."
                .into()
        }
        "net::ERR_NAME_NOT_RESOLVED" => "The page did not load: its site was not found.".into(),
        "net::ERR_CONNECTION_REFUSED" | "net::ERR_CONNECTION_TIMED_OUT" | "net::ERR_TIMED_OUT" => {
            format!("The page did not load: its site did not answer ({error}).")
        }
        _ => format!("The page did not load ({error})."),
    }
}

fn status_error(status: u64) -> String {
    match status {
        401 => "The page asked for a sign-in (HTTP 401). Sign in to the site in the browser beside a chat, then add the page from there.".into(),
        403 => "The site refused to show the page (HTTP 403). It may need a sign-in, or it may turn away automated browsers.".into(),
        404 | 410 => format!("The page was not found (HTTP {status})."),
        429 => "The site asked to slow down (HTTP 429). Try again later.".into(),
        _ => format!("The site answered with an error (HTTP {status})."),
    }
}

fn letters(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

/// The page's readable text, or why it cannot be kept.
pub(crate) fn judge(value: &Value, limits: Limits) -> Result<Read, String> {
    let text = value["text"].as_str().unwrap_or("").trim().to_owned();
    let url = value["url"].as_str().unwrap_or("").to_owned();
    if value["over"] == true || text.chars().count() > limits.text_chars {
        return Err(format!(
            "The page has more text than a notebook source can hold ({} characters).",
            crate::knowledge::extract::thousands(limits.text_chars)
        ));
    }
    let count = letters(&text);
    if value["password"] == true && count < SIGN_IN_TEXT {
        return Err("The page asked for a sign-in. Sign in to the site in the browser beside a chat, then add the page from there.".into());
    }
    if bot_check(&text) {
        return Err("The page showed a check for automated browsers. Open it in the browser beside a chat, get past the check, then add the page from there.".into());
    }
    if count < LEAST_TEXT {
        return Err("The page had no readable text.".into());
    }
    let clean = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = [value["title"].as_str(), value["heading"].as_str()]
        .into_iter()
        .flatten()
        .map(clean)
        .find(|t| !t.is_empty())
        .or_else(|| {
            url::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
        })
        .unwrap_or_else(|| "Web page".into());
    let title: String = title.chars().take(180).collect();
    Ok(Read::Page { url, title, text })
}

/// The expression that reads the page: READABLE called with the most characters.
fn readable(limit: usize) -> String {
    format!("({READABLE})({limit})")
}

/// Reads the page's main content as Markdown: headings as #, list items as -, tables as
/// | rows, code fenced, math as $TeX$. It prefers the page's article or main part, and
/// leaves out what is hidden, fixed to the screen, navigation, forms and the page's own
/// header and footer. `over` says the text passed `limit` characters (it stops there).
const READABLE: &str = r#"(limit) => {
  const skip = new Set(['SCRIPT','STYLE','NOSCRIPT','TEMPLATE','SVG','CANVAS','IFRAME','HEAD','META','LINK','OBJECT','EMBED','NAV','ASIDE','FOOTER','FORM','BUTTON','DIALOG','SELECT','INPUT','TEXTAREA','VIDEO','AUDIO','MAP']);
  const roles = new Set(['navigation','banner','contentinfo','complementary','search','dialog','alertdialog','menu','menubar','toolbar','tablist']);
  const clean = s => (s || '').replace(/\s+/g, ' ').trim();
  const shown = el => { if (el.checkVisibility && !el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) return false; const r = el.getBoundingClientRect(); return r.width > 0 || r.height > 0; };
  const size = el => (el.innerText || '').length;
  const body = document.body;
  const pdf = document.contentType === 'application/pdf';
  const password = [...document.querySelectorAll('input[type=password]')].some(shown);
  const heading = clean(document.querySelector('h1')?.innerText).slice(0, 300);
  const base = { url: location.href, title: clean(document.title).slice(0, 300), heading, pdf, password, over: false, text: '' };
  if (!body || pdf) return base;
  const total = size(body);
  const biggest = list => list.reduce((a, b) => (size(b) > size(a) ? b : a), list[0]);
  let root = body;
  const articles = [...document.querySelectorAll('article')].filter(shown);
  if (articles.length) { const a = biggest(articles); if (size(a) >= 200 && (articles.length === 1 || size(a) >= total / 2)) root = a; }
  const mains = [...document.querySelectorAll('main, [role=main]')].filter(shown);
  if (root === body && mains.length) { const m = biggest(mains); if (size(m) >= 200) root = m; }
  const out = []; let used = 0, over = false, line = '', quote = 0;
  const add = block => { if (over || !block) return; if (used + block.length > limit) { over = true; return; } out.push(block); used += block.length + 2; };
  // A line that would read as a heading or a code fence is kept as text.
  const plain = t => (/^(#|```|~~~)/.test(t) ? '\\' + t : t);
  const flush = () => { const t = clean(line); line = ''; if (t && t !== '-') add((quote ? '> ' : '') + plain(t)); };
  const tex = el => clean(el.getAttribute?.('alttext') || el.querySelector?.('annotation[encoding="application/x-tex"]')?.textContent || el.textContent);
  const walk = node => {
    if (over) return;
    if (node.nodeType === 3) { line += node.nodeValue; return; }
    if (node.nodeType !== 1) return;
    const el = node, tag = el.tagName.toUpperCase();
    if (skip.has(tag) || el.hidden || el.getAttribute('aria-hidden') === 'true' || roles.has(el.getAttribute('role'))) return;
    if (tag === 'HEADER' && root === body && !el.closest('article, main, [role=main]')) return;
    const style = getComputedStyle(el);
    if (style.display === 'contents') { for (const c of el.childNodes) walk(c); return; }
    if (!shown(el) || style.position === 'fixed') return;
    if (tag === 'MATH') { line += ' $' + tex(el) + '$ '; return; }
    if (el.classList.contains('katex-mathml')) { line += ' $' + tex(el) + '$ '; return; }
    if (el.classList.contains('katex-html')) return;
    if (tag === 'MJX-CONTAINER') { const m = el.querySelector('math'); if (m) line += ' $' + tex(m) + '$ '; return; }
    if (/^H[1-6]$/.test(tag)) { flush(); const t = clean(el.innerText); if (t) add('#'.repeat(Number(tag[1])) + ' ' + t); return; }
    if (tag === 'PRE') { flush(); const t = (el.innerText || '').replace(/```/g, "'''").replace(/\s+$/, ''); if (t.trim()) add('```\n' + t + '\n```'); return; }
    if (tag === 'TABLE') {
      flush();
      const rows = [...el.rows].map(r => '| ' + [...r.cells].map(c => clean(c.innerText).replace(/\|/g, '/')).join(' | ') + ' |').filter(r => r.replace(/[|\s]/g, ''));
      if (rows.length) add(rows.join('\n'));
      return;
    }
    if (tag === 'BR') { flush(); return; }
    if (tag === 'IMG') { if (el.alt && el.closest('figure')) line += ` [image: ${clean(el.alt).slice(0, 160)}] `; return; }
    const block = !style.display.startsWith('inline');
    if (block) flush();
    if (tag === 'LI') line = '- ';
    if (tag === 'BLOCKQUOTE') quote++;
    const children = el.shadowRoot ? [...el.shadowRoot.childNodes, ...el.childNodes] : el.childNodes;
    for (const c of children) walk(c);
    if (block) flush();
    if (tag === 'BLOCKQUOTE') quote--;
  };
  walk(root);
  flush();
  return { ...base, over, text: out.join('\n\n') };
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        text_chars: 2000,
        pdf_bytes: 1000,
        public_only: true,
    };

    #[test]
    fn pdfs_are_known_by_type_or_by_a_generic_type_at_a_pdf_address() {
        assert!(is_pdf("application/pdf", "https://example.org/paper"));
        assert!(is_pdf("Application/PDF; qs=0.9", "https://example.org/x"));
        assert!(is_pdf(
            "application/octet-stream",
            "https://example.org/a/B.PDF?x=1"
        ));
        assert!(!is_pdf(
            "application/octet-stream",
            "https://example.org/a/b.zip"
        ));
        assert!(!is_pdf(
            "text/html; charset=utf-8",
            "https://example.org/a.pdf"
        ));
    }

    #[test]
    fn a_pdf_is_named_by_the_site_or_its_address() {
        assert_eq!(
            pdf_name(
                "https://example.org/get?id=4",
                Some("attachment; filename=\"Warmup notes.pdf\"")
            ),
            "Warmup notes.pdf"
        );
        assert_eq!(
            pdf_name("https://example.org/papers/2609.03874v2", None),
            "2609.03874v2.pdf"
        );
        assert_eq!(
            pdf_name("https://example.org/papers/a%20b.pdf", None),
            "a b.pdf"
        );
        assert_eq!(pdf_name("https://example.org/", None), "example.org.pdf");
        assert_eq!(
            pdf_name(
                "https://example.org/x",
                Some("inline; filename=\"../../etc\"")
            ),
            "example.org.pdf"
        );
    }

    #[test]
    fn a_page_is_kept_only_with_readable_text_that_fits() {
        let page = |more: Value| {
            let mut v = json!({"url":"https://example.org/post","title":"  A   post ","heading":"","text":"# Post\n\n".to_owned() + &"word ".repeat(40),"password":false,"over":false});
            if let (Some(v), Value::Object(more)) = (v.as_object_mut(), more) {
                v.extend(more);
            }
            v
        };
        match judge(&page(json!({})), LIMITS).unwrap() {
            Read::Page { title, url, .. } => {
                assert_eq!(
                    (title.as_str(), url.as_str()),
                    ("A post", "https://example.org/post")
                )
            }
            other => panic!("{other:?}"),
        }
        // Without a title: the first heading, then the site.
        let untitled = judge(&page(json!({"title":"","heading":"First heading"})), LIMITS);
        assert!(matches!(untitled, Ok(Read::Page { title, .. }) if title == "First heading"));
        let bare = judge(&page(json!({"title":"","heading":""})), LIMITS);
        assert!(matches!(bare, Ok(Read::Page { title, .. }) if title == "example.org"));
        let cases = [
            (json!({"over":true}), "more text than"),
            (json!({"text":"x".repeat(2001)}), "more text than"),
            (
                json!({"text":"Sign in to read. Password","password":true}),
                "asked for a sign-in",
            ),
            (
                json!({"text":"Please complete the captcha to continue reading this page now"}),
                "check for automated",
            ),
            (json!({"text":"Loading…"}), "no readable text"),
        ];
        for (more, said) in cases {
            let err = judge(&page(more.clone()), LIMITS).unwrap_err();
            assert!(err.contains(said), "{more}: {err}");
        }
        // A long article with a sign-in box beside it is still the article.
        let long = "word ".repeat(400);
        assert!(judge(
            &page(json!({"text":long,"password":true})),
            Limits {
                text_chars: 5000,
                ..LIMITS
            }
        )
        .is_ok());
    }

    #[test]
    fn failures_say_why_in_plain_words() {
        assert!(status_error(401).contains("sign-in"));
        assert!(status_error(404).contains("not found"));
        assert!(status_error(503).contains("HTTP 503"));
        assert!(load_error("net::ERR_ABORTED").contains("download"));
        assert!(load_error("net::ERR_NAME_NOT_RESOLVED").contains("not found"));
        assert!(pdf_too_large(LIMITS).contains("1 KB"));
    }
}

/// Reads real pages in a real Chromium: start one with remote debugging, serve
/// the test pages, then OBLETH_TEST_BROWSER=127.0.0.1:9222
/// OBLETH_TEST_PAGES=http://127.0.0.1:8000 cargo test -- --ignored live_read
#[cfg(test)]
mod live_tests {
    use super::*;

    async fn tab() -> Option<(Cdp, String, String, String)> {
        let address = std::env::var("OBLETH_TEST_BROWSER").ok()?;
        let pages = std::env::var("OBLETH_TEST_PAGES").ok()?;
        let cdp = Cdp::connect(&reqwest::Client::new(), &address)
            .await
            .unwrap();
        let created = cdp
            .call("Target.createTarget", json!({"url":"about:blank"}), None)
            .await
            .unwrap();
        let target = created["targetId"].as_str().unwrap().to_owned();
        let session = cdp.attach(&target).await.unwrap();
        Some((cdp, session, target, pages))
    }

    #[tokio::test]
    #[ignore]
    async fn live_read_pages_and_pdfs() {
        let Some((cdp, session, target, pages)) = tab().await else {
            return;
        };
        let limits = Limits {
            text_chars: 500_000,
            pdf_bytes: 32 << 20,
            // The test pages are served from this machine.
            public_only: false,
        };
        for page in [
            "article.html",
            "moved",
            "script.html",
            "signin.html",
            "missing.html",
            "paper.pdf",
            "file.pdf",
            "data.csv",
        ] {
            let read = read_in(&cdp, &session, &target, &format!("{pages}/{page}"), limits).await;
            match &read {
                Ok(Read::Page { url, title, text }) => {
                    println!("--- {page}: {title} ({url})\n{text}\n")
                }
                Ok(Read::Pdf { url, name, bytes }) => {
                    println!("--- {page}: PDF {name} {} bytes ({url})\n", bytes.len())
                }
                Err(e) => println!("--- {page}: {e}\n"),
            }
        }
        // A tab showing a PDF, as a person would open one: the reader sees the viewer.
        let _ = cdp
            .call(
                "Page.navigate",
                json!({"url":format!("{pages}/paper.pdf")}),
                Some(&session),
            )
            .await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let frame = cdp
            .call("Page.getResourceTree", json!({}), Some(&session))
            .await;
        println!(
            "--- a tab showing paper.pdf has the frame {:?}",
            frame.map(|f| f["frameTree"]["frame"].clone())
        );
        close_scratch(&cdp, &target).await;
    }
}
