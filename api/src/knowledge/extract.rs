//! Turns an uploaded file into pages of plain text, then into passages for search.
//! PDFs go through poppler's `pdftotext` and Word files through `unzip`, each in a
//! short-lived process with a time limit, so a hostile file cannot hang Chat.

use anyhow::{bail, Context, Result};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
};
use uuid::Uuid;

/// Characters per passage: about 300 words, small enough to cite precisely.
const PASSAGE: usize = 1600;
/// Text carried from the end of one passage into the next on the same page.
const OVERLAP: usize = 200;
pub const MAX_PASSAGES: usize = 6000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Docx,
    Html,
    Text,
}

/// What a file name and type say about how to read it; None when unsupported.
pub fn kind(name: &str, mime: &str) -> Option<Kind> {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => Some(Kind::Pdf),
        "docx" => Some(Kind::Docx),
        "html" | "htm" => Some(Kind::Html),
        "txt" | "md" | "markdown" | "rst" | "tex" | "csv" | "tsv" | "json" | "jsonl" | "yaml"
        | "yml" | "toml" | "xml" | "ipynb" | "py" | "rs" | "go" | "java" | "c" | "h" | "cc"
        | "cpp" | "hpp" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "sh" | "sql" | "r" | "jl" | "m"
        | "f90" | "cu" | "log" => Some(Kind::Text),
        _ if mime == "application/pdf" => Some(Kind::Pdf),
        _ if mime.starts_with("text/") => Some(Kind::Text),
        _ => None,
    }
}

/// The type a file is stored with, from its kind alone: the uploader's own type is
/// never kept. Web pages are stored as text, which is all Chat reads from them.
pub fn mime_for(kind: Kind) -> String {
    match kind {
        Kind::Pdf => "application/pdf".into(),
        Kind::Docx => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into()
        }
        Kind::Html | Kind::Text => "text/plain; charset=utf-8".into(),
    }
}

pub struct Page {
    /// From 1 for PDFs; None for formats without pages.
    pub number: Option<i32>,
    pub text: String,
}

pub enum Extracted {
    Pages(Vec<Page>),
    /// A PDF whose pages are images: it needs text recognition first.
    NeedsOcr {
        pages: i32,
    },
}

#[derive(Clone)]
pub struct Tools {
    pub pdftotext: String,
    pub unzip: String,
}

impl Tools {
    pub fn from_env() -> Self {
        let var = |key: &str, default: &str| {
            std::env::var(key)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| default.into())
        };
        Self {
            pdftotext: var("OBLETH_CHAT_PDFTOTEXT", "pdftotext"),
            unzip: var("OBLETH_CHAT_UNZIP", "unzip"),
        }
    }
}

pub async fn extract(tools: &Tools, kind: Kind, bytes: &[u8]) -> Result<Extracted> {
    match kind {
        Kind::Text => Ok(Extracted::Pages(vec![Page {
            number: None,
            text: String::from_utf8_lossy(bytes).into_owned(),
        }])),
        Kind::Html => Ok(Extracted::Pages(vec![Page {
            number: None,
            text: html_text(&String::from_utf8_lossy(bytes)),
        }])),
        Kind::Docx => {
            let xml = run(&tools.unzip, &["-p", "{file}", "word/document.xml"], bytes)
                .await
                .context("this Word file could not be opened")?;
            Ok(Extracted::Pages(vec![Page {
                number: None,
                text: docx_text(&String::from_utf8_lossy(&xml)),
            }]))
        }
        Kind::Pdf => {
            let out = run(
                &tools.pdftotext,
                &["-enc", "UTF-8", "-q", "{file}", "-"],
                bytes,
            )
            .await
            .context("this PDF could not be read")?;
            let text = String::from_utf8_lossy(&out);
            // pdftotext ends every page with a form feed.
            let mut pages: Vec<&str> = text.split('\u{c}').collect();
            if pages.last().is_some_and(|p| p.trim().is_empty()) {
                pages.pop();
            }
            let count = pages.len().max(1) as i32;
            let letters = text.chars().filter(|c| c.is_alphanumeric()).count();
            if letters < 40 * count as usize {
                return Ok(Extracted::NeedsOcr { pages: count });
            }
            Ok(Extracted::Pages(
                pages
                    .into_iter()
                    .enumerate()
                    .map(|(i, text)| Page {
                        number: Some(i as i32 + 1),
                        text: text.to_owned(),
                    })
                    .collect(),
            ))
        }
    }
}

/// Runs a tool on the file, given as `{file}` in the arguments, and returns stdout.
/// Output is read as it comes and the tool is stopped once it passes MAX_OUTPUT, so a
/// small file that expands enormously (a zip bomb) cannot exhaust memory.
async fn run(program: &str, args: &[&str], bytes: &[u8]) -> Result<Vec<u8>> {
    let path: PathBuf = std::env::temp_dir().join(format!("obleth-knowledge-{}", Uuid::new_v4()));
    tokio::fs::write(&path, bytes)
        .await
        .context("writing a temporary file")?;
    let file = path.to_string_lossy().into_owned();
    let args: Vec<String> = args.iter().map(|a| a.replace("{file}", &file)).collect();
    let child = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let result = match child {
        Ok(child) => {
            match tokio::time::timeout(Duration::from_secs(180), output(program, child)).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("{program} took too long")),
            }
        }
        Err(error) => Err(anyhow::anyhow!("{program} is not available: {error}")),
    };
    let _ = tokio::fs::remove_file(&path).await;
    result
}

/// Most output a tool may write: more text than Chat indexes from one file.
const MAX_OUTPUT: usize = 64 * 1024 * 1024;

async fn output(program: &str, mut child: Child) -> Result<Vec<u8>> {
    let mut stdout = child.stdout.take().context("reading the tool's output")?;
    let mut out = Vec::new();
    let mut chunk = vec![0; 64 * 1024];
    loop {
        let n = stdout.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        if out.len() + n > MAX_OUTPUT {
            let _ = child.kill().await;
            bail!("the file holds more text than Chat can index");
        }
        out.extend_from_slice(&chunk[..n]);
    }
    let status = child.wait().await?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(out)
}

fn docx_text(xml: &str) -> String {
    let xml = xml
        .replace("</w:p>", "\n\n")
        .replace("<w:tab/>", "\t")
        .replace("<w:br/>", "\n")
        .replace("<w:cr/>", "\n");
    decode(&strip_tags(&xml))
}

fn html_text(html: &str) -> String {
    let mut text = html.to_owned();
    for tag in ["script", "style", "noscript", "template"] {
        text = remove_blocks(&text, tag);
    }
    for close in [
        "</p>",
        "</div>",
        "</li>",
        "</tr>",
        "</h1>",
        "</h2>",
        "</h3>",
        "</h4>",
        "</h5>",
        "</h6>",
        "</section>",
        "</article>",
        "</pre>",
        "</blockquote>",
    ] {
        text = replace_ci(&text, close, "\n\n");
    }
    text = replace_ci(&text, "<br>", "\n");
    text = replace_ci(&text, "<br/>", "\n");
    text = replace_ci(&text, "<br />", "\n");
    decode(&strip_tags(&text))
}

fn replace_ci(text: &str, from: &str, to: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in lower.match_indices(from) {
        out.push_str(&text[last..i]);
        out.push_str(to);
        last = i + from.len();
    }
    out.push_str(&text[last..]);
    out
}

fn remove_blocks(text: &str, tag: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(start) = lower[at..].find(&open).map(|i| at + i) {
        out.push_str(&text[at..start]);
        match lower[start..].find(&close) {
            Some(end) => at = start + end + close.len(),
            None => {
                at = text.len();
                break;
            }
        }
    }
    out.push_str(&text[at.min(text.len())..]);
    out
}

fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for c in text.chars() {
        match c {
            '<' => inside = true,
            '>' if inside => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[..rest.len().min(12)].find(';');
        let decoded = end.and_then(|end| {
            let entity = &rest[1..end];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub struct Passage {
    pub page: Option<i32>,
    pub text: String,
}

/// Splits pages into passages of about PASSAGE characters along paragraph and sentence
/// boundaries. A passage never spans pages, so a citation names one page.
pub fn split(pages: &[Page]) -> Vec<Passage> {
    let mut passages = Vec::new();
    for page in pages {
        let paragraphs = paragraphs(&page.text);
        let mut current = String::new();
        let push = |current: &mut String, passages: &mut Vec<Passage>| {
            let text = current.trim().to_owned();
            if text.chars().filter(|c| c.is_alphanumeric()).count() >= 20 {
                passages.push(Passage {
                    page: page.number,
                    text,
                });
            }
            // Carry the end of this passage into the next for context.
            let tail = tail(current, OVERLAP);
            *current = tail;
        };
        for paragraph in paragraphs {
            for piece in pieces(&paragraph, PASSAGE) {
                if current.len() + piece.len() + 2 > PASSAGE && current.len() > OVERLAP {
                    push(&mut current, &mut passages);
                }
                if !current.is_empty() {
                    current.push_str("\n\n");
                }
                current.push_str(&piece);
            }
        }
        if current.len() > OVERLAP || passages.last().map(|p| p.page) != Some(page.number) {
            push(&mut current, &mut passages);
        }
        if passages.len() >= MAX_PASSAGES {
            passages.truncate(MAX_PASSAGES);
            break;
        }
    }
    passages
}

/// Paragraphs with whitespace tidied; single line breaks inside a paragraph become spaces.
fn paragraphs(text: &str) -> Vec<String> {
    text.replace('\r', "")
        .split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect()
}

/// Splits an over-long paragraph at sentence ends, then at spaces.
fn pieces(paragraph: &str, size: usize) -> Vec<String> {
    if paragraph.len() <= size {
        return vec![paragraph.to_owned()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for sentence in paragraph.split_inclusive(['.', '?', '!']) {
        for word in sentence.split_inclusive(' ') {
            if current.len() + word.len() > size && !current.is_empty() {
                out.push(current.trim().to_owned());
                current.clear();
            }
            current.push_str(word);
        }
        if current.len() > size * 3 / 4 {
            out.push(current.trim().to_owned());
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_owned());
    }
    out
}

/// The last `n` bytes of text, starting at a word boundary.
fn tail(text: &str, n: usize) -> String {
    if text.len() <= n {
        return String::new();
    }
    let mut start = text.len() - n;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let rest = &text[start..];
    rest.split_once(' ')
        .map(|(_, r)| r)
        .unwrap_or(rest)
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_the_file_name() {
        assert_eq!(
            kind("Lecture 7.PDF", "application/octet-stream"),
            Some(Kind::Pdf)
        );
        assert_eq!(kind("notes.md", ""), Some(Kind::Text));
        assert_eq!(kind("report.docx", ""), Some(Kind::Docx));
        assert_eq!(kind("photo.jpg", "image/jpeg"), None);
    }

    #[test]
    fn html_and_word_become_plain_text() {
        let html = "<html><style>p{}</style><body><h1>Title</h1><p>One &amp; two</p><script>x()</script><p>Three&#33;</p></body></html>";
        let text = html_text(html);
        assert!(
            text.contains("Title") && text.contains("One & two") && text.contains("Three!"),
            "{text}"
        );
        assert!(!text.contains("x()") && !text.contains("p{}"));
        let docx = "<w:document><w:body><w:p><w:r><w:t>First</w:t></w:r></w:p><w:p><w:r><w:t>Second &lt;b&gt;</w:t></w:r></w:p></w:body></w:document>";
        assert_eq!(
            docx_text(docx).split_whitespace().collect::<Vec<_>>(),
            ["First", "Second", "<b>"]
        );
    }

    #[test]
    fn stored_types_ignore_what_the_uploader_said() {
        assert_eq!(kind("notes.txt", "image/svg+xml; x=json"), Some(Kind::Text));
        assert_eq!(mime_for(Kind::Text), "text/plain; charset=utf-8");
        assert_eq!(mime_for(Kind::Html), "text/plain; charset=utf-8");
        assert_eq!(mime_for(Kind::Pdf), "application/pdf");
    }

    #[tokio::test]
    async fn a_tool_is_stopped_once_its_output_passes_the_cap() {
        let marker = std::env::temp_dir().join(format!("obleth-knowledge-{}", Uuid::new_v4()));
        let script = format!(
            "head -c {} /dev/zero; touch '{}'",
            MAX_OUTPUT + 4 * 1024 * 1024,
            marker.display()
        );
        let error = tokio::time::timeout(Duration::from_secs(30), run("sh", &["-c", &script], b""))
            .await
            .expect("the tool was not stopped")
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("more text than Chat can index"),
            "{error:#}"
        );
        // The tool was killed mid-output instead of being read to the end.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!marker.exists());
    }

    /// A Word file whose document.xml inflates from about half a megabyte to past the
    /// output cap: fixed-Huffman deflate of one zero byte followed by 258-byte copies.
    fn docx_bomb() -> Vec<u8> {
        let copies = (MAX_OUTPUT + 8 * 1024 * 1024) / 258;
        let size = (1 + copies * 258) as u32;
        let (mut deflated, mut bits, mut used) = (Vec::new(), 0u32, 0u32);
        let mut put = |value: u32, count: u32, out: &mut Vec<u8>| {
            bits |= value << used;
            used += count;
            while used >= 8 {
                out.push(bits as u8);
                bits >>= 8;
                used -= 8;
            }
        };
        // Huffman codes go most significant bit first.
        let reversed = |code: u32, count: u32| code.reverse_bits() >> (32 - count);
        put(0b011, 3, &mut deflated); // last block, fixed codes
        put(reversed(0x30, 8), 8, &mut deflated); // literal 0
        for _ in 0..copies {
            put(reversed(0xc5, 8), 8, &mut deflated); // length 258
            put(0, 5, &mut deflated); // distance 1
        }
        put(0, 7, &mut deflated); // end of block
        put(0, 7, &mut deflated);
        // CRC-32 of the inflated zeros, a byte at a time.
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            *entry = (0..8).fold(i as u32, |c, _| {
                if c & 1 == 1 {
                    (c >> 1) ^ 0xedb8_8320
                } else {
                    c >> 1
                }
            });
        }
        let crc = !(0..size).fold(!0u32, |c, _| table[(c & 0xff) as usize] ^ (c >> 8));
        let name = b"word/document.xml";
        let entry = |zip: &mut Vec<u8>, signature: u32, central: bool| {
            zip.extend(signature.to_le_bytes());
            if central {
                zip.extend(20u16.to_le_bytes());
            }
            zip.extend(20u16.to_le_bytes());
            zip.extend(0u16.to_le_bytes()); // flags
            zip.extend(8u16.to_le_bytes()); // deflate
            zip.extend(0u32.to_le_bytes()); // time and date
            zip.extend(crc.to_le_bytes());
            zip.extend((deflated.len() as u32).to_le_bytes());
            zip.extend(size.to_le_bytes());
            zip.extend((name.len() as u16).to_le_bytes());
            zip.extend(0u16.to_le_bytes()); // extra
            if central {
                zip.extend([0u8; 10]); // comment, disk, attributes
                zip.extend(0u32.to_le_bytes()); // local header offset
            }
            zip.extend(name);
        };
        let mut zip = Vec::new();
        entry(&mut zip, 0x0403_4b50, false);
        zip.extend(&deflated);
        let directory = zip.len() as u32;
        entry(&mut zip, 0x0201_4b50, true);
        let directory_size = zip.len() as u32 - directory;
        zip.extend(0x0605_4b50u32.to_le_bytes());
        zip.extend([0u8; 4]);
        zip.extend(1u16.to_le_bytes());
        zip.extend(1u16.to_le_bytes());
        zip.extend(directory_size.to_le_bytes());
        zip.extend(directory.to_le_bytes());
        zip.extend(0u16.to_le_bytes());
        zip
    }

    #[tokio::test]
    async fn a_word_file_that_expands_past_the_cap_fails_cleanly() {
        let tools = Tools::from_env();
        if std::process::Command::new(&tools.unzip)
            .arg("-v")
            .output()
            .is_err()
        {
            eprintln!("skipping: {} is not available", tools.unzip);
            return;
        }
        let bomb = docx_bomb();
        assert!(bomb.len() < 1024 * 1024, "{}", bomb.len());
        let error =
            match tokio::time::timeout(Duration::from_secs(60), extract(&tools, Kind::Docx, &bomb))
                .await
                .expect("extraction was not stopped")
            {
                Ok(_) => panic!("a file past the cap was extracted"),
                Err(error) => format!("{error:#}"),
            };
        assert!(
            error.contains("this Word file could not be opened")
                && error.contains("more text than Chat can index"),
            "{error}"
        );
    }

    #[test]
    fn passages_stay_on_their_page_and_overlap() {
        let long = "Entropy is a measure of the number of microstates. ".repeat(80);
        let pages = vec![
            Page {
                number: Some(1),
                text: format!("Intro paragraph about heat engines and work.\n\n{long}"),
            },
            Page {
                number: Some(2),
                text: "A short second page about the second law of thermodynamics.".into(),
            },
        ];
        let passages = split(&pages);
        assert!(passages.len() >= 3, "{}", passages.len());
        assert!(passages
            .iter()
            .all(|p| p.text.len() <= PASSAGE + OVERLAP + 4));
        assert_eq!(passages.last().unwrap().page, Some(2));
        assert!(passages.iter().filter(|p| p.page == Some(2)).count() == 1);
        // The second passage on page 1 starts with text from the end of the first.
        let (a, b) = (&passages[0].text, &passages[1].text);
        assert!(
            a.ends_with(&b[..20.min(b.len())]) || a.contains(&b[..20]),
            "no overlap"
        );
    }
}
