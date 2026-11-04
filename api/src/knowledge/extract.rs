//! Turns an uploaded file into pages of plain text, then into passages for search.
//! PDFs go through poppler's `pdftotext` (and `pdftohtml` for their bookmarks) and Word,
//! PowerPoint and Excel files through `unzip`, each in a short-lived process with a time
//! limit, so a hostile file cannot hang Chat.
//!
//! Each part of a file keeps the headings it sits under, its section: HTML headings,
//! Markdown `#` lines, Word heading styles, PDF bookmarks, slides and sheets. A passage
//! never spans two sections, so a citation names one.

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
/// Spreadsheet cells read from one workbook; the text says where it stopped.
pub const MAX_CELLS: usize = 100_000;
/// Columns read from one row, so a stray cell far to the right can't pad every row.
const MAX_COLUMNS: usize = 256;
pub const MAX_SLIDES: usize = 1000;
const MAX_SHEETS: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Html,
    Markdown,
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
        "pptx" => Some(Kind::Pptx),
        "xlsx" => Some(Kind::Xlsx),
        "html" | "htm" => Some(Kind::Html),
        "md" | "markdown" => Some(Kind::Markdown),
        "txt" | "rst" | "tex" | "csv" | "tsv" | "json" | "jsonl" | "yaml" | "yml" | "toml"
        | "xml" | "ipynb" | "py" | "rs" | "go" | "java" | "c" | "h" | "cc" | "cpp" | "hpp"
        | "js" | "mjs" | "ts" | "tsx" | "jsx" | "sh" | "sql" | "r" | "jl" | "m" | "f90" | "cu"
        | "log" => Some(Kind::Text),
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
        Kind::Pptx => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation".into()
        }
        Kind::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
        Kind::Html | Kind::Markdown | Kind::Text => "text/plain; charset=utf-8".into(),
    }
}

pub struct Page {
    /// From 1 for PDFs; None for formats without pages.
    pub number: Option<i32>,
    /// The headings this text sits under, outermost first, joined with " › ".
    pub section: Option<String>,
    pub text: String,
}

impl Page {
    fn whole(text: String) -> Self {
        Self {
            number: None,
            section: None,
            text,
        }
    }

    /// One page per section; a document with no headings is one page.
    fn sections(sections: Vec<Section>, whole: impl FnOnce() -> String) -> Vec<Self> {
        if !sections.iter().any(|s| !s.path.is_empty()) {
            return vec![Self::whole(whole())];
        }
        sections
            .into_iter()
            .map(|s| Self {
                number: None,
                section: (!s.path.is_empty()).then(|| s.path.join(" › ")),
                text: s.text,
            })
            .collect()
    }
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
    /// Reads a PDF's bookmarks (`pdftohtml -xml`).
    pub pdftohtml: String,
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
            pdftohtml: var("OBLETH_CHAT_PDFTOHTML", "pdftohtml"),
            unzip: var("OBLETH_CHAT_UNZIP", "unzip"),
        }
    }
}

pub async fn extract(tools: &Tools, kind: Kind, bytes: &[u8]) -> Result<Extracted> {
    match kind {
        Kind::Text => Ok(Extracted::Pages(vec![Page::whole(
            String::from_utf8_lossy(bytes).into_owned(),
        )])),
        Kind::Markdown => {
            let text = String::from_utf8_lossy(bytes);
            Ok(Extracted::Pages(Page::sections(
                markdown_sections(&text).1,
                || text.to_string(),
            )))
        }
        Kind::Html => {
            let html = String::from_utf8_lossy(bytes);
            Ok(Extracted::Pages(Page::sections(
                html_sections(&html).1,
                || html_text(&html),
            )))
        }
        Kind::Docx => {
            let mut zip = Zip::new(&tools.unzip, bytes).await?;
            let xml = zip
                .read("word/document.xml")
                .await
                .context("this Word file could not be opened")?;
            // Heading styles are named in styles.xml; a file without one has no headings.
            let styles = zip.read("word/styles.xml").await.unwrap_or_default();
            Ok(Extracted::Pages(Page::sections(
                docx_sections(&xml, &styles).1,
                || docx_text(&xml),
            )))
        }
        Kind::Pptx => {
            let mut zip = Zip::new(&tools.unzip, bytes).await?;
            Ok(Extracted::Pages(
                pptx_pages(&mut zip)
                    .await
                    .context("this PowerPoint file could not be opened")?,
            ))
        }
        Kind::Xlsx => {
            let mut zip = Zip::new(&tools.unzip, bytes).await?;
            Ok(Extracted::Pages(
                xlsx_pages(&mut zip)
                    .await
                    .context("this Excel file could not be opened")?,
            ))
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
            // Bookmarks are a bonus: a PDF without them, or a pdftohtml that fails, is
            // read without sections.
            let outline = run(
                &tools.pdftohtml,
                &["-xml", "-i", "-q", "-stdout", "{file}"],
                bytes,
            )
            .await
            .map(|xml| pdf_outline(&String::from_utf8_lossy(&xml)))
            .unwrap_or_default();
            Ok(Extracted::Pages(pdf_pages(&pages, &outline)))
        }
    }
}

/// Runs a tool on the file, given as `{file}` in the arguments, and returns stdout.
async fn run(program: &str, args: &[&str], bytes: &[u8]) -> Result<Vec<u8>> {
    let file = TempFile::new(bytes).await?;
    run_on(program, args, &file, MAX_OUTPUT).await
}

/// Runs a tool on a file already written out. Output is read as it comes and the tool
/// is stopped once it passes `limit`, so a small file that expands enormously (a zip
/// bomb) cannot exhaust memory.
async fn run_on(program: &str, args: &[&str], file: &TempFile, limit: usize) -> Result<Vec<u8>> {
    let path = file.0.to_string_lossy();
    let args: Vec<String> = args.iter().map(|a| a.replace("{file}", &path)).collect();
    let child = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    match child {
        Ok(child) => {
            match tokio::time::timeout(Duration::from_secs(180), output(program, child, limit))
                .await
            {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("{program} took too long")),
            }
        }
        Err(error) => Err(anyhow::anyhow!("{program} is not available: {error}")),
    }
}

/// An uploaded file written to a temporary path for the tools; removed when dropped.
struct TempFile(PathBuf);

impl TempFile {
    async fn new(bytes: &[u8]) -> Result<Self> {
        let path = std::env::temp_dir().join(format!("obleth-knowledge-{}", Uuid::new_v4()));
        tokio::fs::write(&path, bytes)
            .await
            .context("writing a temporary file")?;
        Ok(Self(path))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Most output a tool may write: more text than Chat indexes from one file.
const MAX_OUTPUT: usize = 64 * 1024 * 1024;

async fn output(program: &str, mut child: Child, limit: usize) -> Result<Vec<u8>> {
    let mut stdout = child.stdout.take().context("reading the tool's output")?;
    let mut out = Vec::new();
    let mut chunk = vec![0; 64 * 1024];
    loop {
        let n = stdout.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        if out.len() + n > limit {
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

/// An Office file (a zip archive) whose parts are read one at a time with `unzip -p`.
/// Every part read counts against one budget of MAX_OUTPUT, so many parts can't add up
/// to more than one oversized part could.
struct Zip<'a> {
    unzip: &'a str,
    file: TempFile,
    left: usize,
}

impl<'a> Zip<'a> {
    async fn new(unzip: &'a str, bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            unzip,
            file: TempFile::new(bytes).await?,
            left: MAX_OUTPUT,
        })
    }

    async fn read(&mut self, part: &str) -> Result<String> {
        // unzip reads its arguments as patterns: a part named by the file itself must
        // be a plain path, or it could match every part at once.
        if part.is_empty()
            || part.split('/').any(|s| s.is_empty() || s == "..")
            || !part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "/_-.".contains(c))
        {
            bail!("the file refers to a part named {part:?}");
        }
        let out = run_on(self.unzip, &["-p", "{file}", part], &self.file, self.left).await?;
        self.left -= out.len();
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

/// The relationships in an Office `.rels` part: each one's id, type and target.
fn relationships(xml: &str) -> Vec<(String, String, String)> {
    elements(xml, "Relationship")
        .into_iter()
        .filter_map(|e| {
            Some((
                attribute(e.head, "Id")?.to_owned(),
                attribute(e.head, "Type").unwrap_or("").to_owned(),
                decode(attribute(e.head, "Target")?),
            ))
        })
        .collect()
}

/// A relationship's target as a path in the archive, from the folder of the part that
/// names it.
fn resolve(folder: &str, target: &str) -> String {
    let mut parts: Vec<&str> = match target.strip_prefix('/') {
        Some(_) => vec![],
        None => folder.split('/').filter(|p| !p.is_empty()).collect(),
    };
    for piece in target.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(piece),
        }
    }
    parts.join("/")
}

/// Where a part's own relationships are kept: `a/b.xml` has `a/_rels/b.xml.rels`.
fn rels_of(part: &str) -> (String, String) {
    let (folder, name) = part.rsplit_once('/').unwrap_or(("", part));
    (folder.to_owned(), format!("{folder}/_rels/{name}.rels"))
}

/// A presentation's slides in order, one page each, under "Slide N" and the slide's
/// title. Speaker notes follow each slide's own text.
async fn pptx_pages(zip: &mut Zip<'_>) -> Result<Vec<Page>> {
    let presentation = zip.read("ppt/presentation.xml").await?;
    let rels = relationships(
        &zip.read("ppt/_rels/presentation.xml.rels")
            .await
            .unwrap_or_default(),
    );
    let slides: Vec<String> = elements(&presentation, "p:sldId")
        .into_iter()
        .filter_map(|e| attribute(e.head, "r:id"))
        .filter_map(|id| rels.iter().find(|(rid, _, _)| rid == id))
        .map(|(_, _, target)| resolve("ppt", target))
        .collect();
    let mut pages = Vec::new();
    for (i, part) in slides.iter().take(MAX_SLIDES).enumerate() {
        let xml = zip.read(part).await?;
        let (title, mut text) = slide_text(&xml);
        let (folder, rels) = rels_of(part);
        let notes = relationships(&zip.read(&rels).await.unwrap_or_default())
            .into_iter()
            .find(|(_, kind, _)| kind.ends_with("/notesSlide"))
            .map(|(_, _, target)| resolve(&folder, &target));
        if let Some(notes) = notes {
            let notes = notes_text(&zip.read(&notes).await.unwrap_or_default());
            if !notes.is_empty() {
                text.push_str(&format!("\n\nSpeaker notes:\n{notes}"));
            }
        }
        let n = i + 1;
        pages.push(Page {
            number: None,
            section: Some(match title {
                Some(title) => format!("Slide {n}: {title}"),
                None => format!("Slide {n}"),
            }),
            text,
        });
    }
    if slides.len() > MAX_SLIDES {
        if let Some(last) = pages.last_mut() {
            last.text.push_str(&format!(
                "\n\n[Chat reads the first {} slides; this file has {}.]",
                thousands(MAX_SLIDES),
                thousands(slides.len())
            ));
        }
    }
    Ok(pages)
}

/// A slide's title, from its title placeholder, and the rest of its text in the order
/// the slide holds it, tables included.
fn slide_text(xml: &str) -> (Option<String>, String) {
    let title = elements(xml, "p:sp")
        .into_iter()
        .find(|shape| matches!(placeholder(shape.body), Some("title" | "ctrTitle")));
    let rest = match &title {
        Some(shape) => format!("{}{}", &xml[..shape.start], &xml[shape.end..]),
        None => xml.to_owned(),
    };
    let title = title
        .map(|shape| one_line(&drawing_text(shape.body)))
        .filter(|t| !t.is_empty());
    (title, drawing_text(&rest))
}

/// The notes on a notes page: its body placeholder, without the slide picture or number.
fn notes_text(xml: &str) -> String {
    elements(xml, "p:sp")
        .into_iter()
        .filter(|shape| placeholder(shape.body) == Some("body"))
        .map(|shape| drawing_text(shape.body))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The type of the placeholder a shape fills, such as "title" or "body".
fn placeholder(shape: &str) -> Option<&str> {
    let ph = elements(shape, "p:ph").into_iter().next()?;
    // A placeholder without a type is a body.
    Some(attribute(ph.head, "type").unwrap_or("body"))
}

/// Text of DrawingML: a line per paragraph, table cells separated with |.
fn drawing_text(xml: &str) -> String {
    // A cell's paragraphs end just before the cell does; the marker joins them.
    let xml = xml
        .replace("</a:tc>", "\u{1f}")
        .replace("</a:p>", "\n")
        .replace("<a:br", "\n<a:br")
        .replace("</a:tr>", "\n");
    decode(&strip_tags(&xml))
        .replace("\n\u{1f}", " | ")
        .replace('\u{1f}', " | ")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A workbook's sheets in order, one page each under "Sheet: name", as comma-separated
/// rows. At most MAX_CELLS cells are read in all; a sheet cut short says so.
async fn xlsx_pages(zip: &mut Zip<'_>) -> Result<Vec<Page>> {
    let workbook = zip.read("xl/workbook.xml").await?;
    let rels = relationships(
        &zip.read("xl/_rels/workbook.xml.rels")
            .await
            .unwrap_or_default(),
    );
    let strings = shared_strings(&zip.read("xl/sharedStrings.xml").await.unwrap_or_default());
    let sheets: Vec<(String, String)> = elements(&workbook, "sheet")
        .into_iter()
        .filter_map(|e| {
            let name = decode(attribute(e.head, "name")?);
            let id = attribute(e.head, "r:id")?;
            let (_, _, target) = rels.iter().find(|(rid, _, _)| rid == id)?;
            Some((name, resolve("xl", target)))
        })
        .collect();
    let mut left = MAX_CELLS;
    let mut pages: Vec<Page> = Vec::new();
    for (k, (name, part)) in sheets.iter().take(MAX_SHEETS).enumerate() {
        if left == 0 {
            let names: Vec<&str> = sheets[k..].iter().map(|(n, _)| n.as_str()).collect();
            if let Some(last) = pages.last_mut() {
                last.text.push_str(&format!(
                    "\n\n[Not included: the sheets {}.]",
                    names.join(", ")
                ));
            }
            break;
        }
        let xml = zip.read(part).await?;
        pages.push(Page {
            number: None,
            section: Some(format!("Sheet: {name}")),
            text: sheet_text(&xml, &strings, &mut left),
        });
    }
    Ok(pages)
}

/// The workbook's shared strings, which cells refer to by index.
fn shared_strings(xml: &str) -> Vec<String> {
    elements(xml, "si")
        .into_iter()
        // Phonetic guides (rPh) repeat the text in another script.
        .map(|e| decode(&strip_tags(&remove_blocks(e.body, "rph"))))
        .collect()
}

/// A sheet's rows as comma-separated values, quoted where needed. Empty rows are left
/// out. Counts the cells read against `left` and says where it stopped.
fn sheet_text(xml: &str, strings: &[String], left: &mut usize) -> String {
    let rows = elements(xml, "row");
    let mut lines = Vec::new();
    for (k, row) in rows.iter().enumerate() {
        if *left == 0 {
            let more = match rows.len() - k {
                1 => "1 more row is".to_owned(),
                n => format!("{n} more rows are"),
            };
            lines.push(format!(
                "[Cut here: {more} not included. Chat reads up to {} cells of a workbook.]",
                thousands(MAX_CELLS)
            ));
            break;
        }
        let mut cells: Vec<String> = Vec::new();
        for cell in elements(row.body, "c") {
            let column = attribute(cell.head, "r")
                .and_then(column_index)
                .unwrap_or(cells.len());
            if column >= MAX_COLUMNS {
                break;
            }
            let value = cell_value(cell.head, cell.body, strings);
            if value.is_empty() {
                continue;
            }
            cells.resize(column.max(cells.len()), String::new());
            cells.push(csv_field(&value));
            *left = left.saturating_sub(1);
        }
        if !cells.is_empty() {
            lines.push(cells.join(","));
        }
    }
    lines.join("\n")
}

/// A cell's shown value: shared and inline strings, booleans, and otherwise the stored
/// value (numbers, a formula's last result, errors).
fn cell_value(head: &str, body: &str, strings: &[String]) -> String {
    let value = || {
        elements(body, "v")
            .first()
            .map(|v| decode(v.body))
            .unwrap_or_default()
    };
    match attribute(head, "t") {
        Some("s") => value()
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|i| strings.get(i).cloned())
            .unwrap_or_default(),
        Some("inlineStr") => decode(&strip_tags(&remove_blocks(body, "rph"))),
        Some("b") => match value().trim() {
            "1" => "TRUE".into(),
            _ => "FALSE".into(),
        },
        _ => value(),
    }
}

/// The column of a cell reference, from 0: "A1" is 0, "AB7" is 27.
fn column_index(reference: &str) -> Option<usize> {
    let letters: Vec<u8> = reference
        .bytes()
        .take_while(u8::is_ascii_alphabetic)
        .collect();
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let n = letters.iter().fold(0usize, |n, c| {
        n * 26 + (c.to_ascii_uppercase() - b'A') as usize + 1
    });
    Some(n - 1)
}

/// 100000 as "100,000".
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let groups: Vec<&str> = digits
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|g| std::str::from_utf8(g).unwrap_or_default())
        .collect();
    groups.join(",")
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// An XML element found by name: where it starts and ends, its opening tag and its
/// content (empty when it closes itself).
struct Element<'a> {
    start: usize,
    end: usize,
    head: &'a str,
    body: &'a str,
}

/// Each `<tag …>` element in order. Elements of the same name must not nest, which
/// holds for every element read with it.
fn elements<'a>(xml: &'a str, tag: &str) -> Vec<Element<'a>> {
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(start) = xml[at..].find(&open).map(|i| at + i) {
        let after = xml.as_bytes().get(start + open.len()).copied();
        if !after.is_some_and(|c| c == b'>' || c == b'/' || c.is_ascii_whitespace()) {
            at = start + open.len();
            continue;
        }
        let Some(open_end) = tag_end(xml, start) else {
            break;
        };
        let head = &xml[start..open_end];
        if head.ends_with("/>") {
            out.push(Element {
                start,
                end: open_end,
                head,
                body: "",
            });
            at = open_end;
            continue;
        }
        let stop = xml[open_end..]
            .find(&close)
            .map_or(xml.len(), |i| open_end + i);
        let end = (stop + close.len()).min(xml.len());
        out.push(Element {
            start,
            end,
            head,
            body: &xml[open_end..stop],
        });
        at = end;
    }
    out
}

/// The pages' text for a model to read whole, with a line in brackets wherever a page
/// or section starts: "[Page 3]", "[Slide 2: Results]", "[Sheet: Budget]".
pub fn marked_text(pages: &[Page]) -> String {
    let mut out = String::new();
    let mut last: Option<(Option<i32>, Option<&str>)> = None;
    for page in pages {
        let text = page.text.trim();
        if text.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let here = (page.number, page.section.as_deref());
        if last != Some(here) {
            let marker = match here {
                (Some(n), Some(section)) => format!("[Page {n} · {section}]\n"),
                (Some(n), None) => format!("[Page {n}]\n"),
                (None, Some(section)) => format!("[{section}]\n"),
                (None, None) => String::new(),
            };
            out.push_str(&marker);
            last = Some(here);
        }
        out.push_str(text);
    }
    out
}

/// How many pages, slides or sheets the file has, for showing next to its name;
/// None for formats without them.
pub fn parts(kind: Kind, pages: &[Page]) -> Option<i32> {
    match kind {
        Kind::Pdf => pages.iter().filter_map(|p| p.number).max(),
        Kind::Pptx | Kind::Xlsx => Some(pages.len() as i32),
        _ => None,
    }
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

/// A part of a document under one heading.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// The heading's level, 1 to 6; 0 for text before the first heading.
    pub level: u8,
    /// The headings above it and its own, outermost first; empty for text before the
    /// first heading or directly under the title.
    pub path: Vec<String>,
    pub text: String,
}

/// A heading and the text under it, before headings are nested into paths.
struct Heading {
    /// 0 for a title, then 1 to 6.
    level: u8,
    title: String,
    text: String,
}

/// Nests headings into sections. A document's title (a title style, or with
/// `lone_title` its only level-1 heading) is kept apart, so it doesn't head every
/// path; the text under it joins the text before the first heading. No text is dropped.
fn nest(lead: String, heads: Vec<Heading>, lone_title: bool) -> (Option<String>, Vec<Section>) {
    let at_title = heads.iter().position(|h| h.level == 0).or_else(|| {
        match heads.iter().filter(|h| h.level == 1).count() {
            1 if lone_title => heads.iter().position(|h| h.level == 1),
            _ => None,
        }
    });
    let title = at_title.map(|t| heads[t].title.clone());
    let mut sections: Vec<Section> = Vec::new();
    let mut push = |level: u8, path: Vec<String>, text: String| {
        let lead = path.is_empty();
        if lead && !text.chars().any(char::is_alphanumeric) {
            return;
        }
        match sections.last_mut() {
            // Text before the title and text under it are one lead.
            Some(last) if lead && last.path.is_empty() => {
                last.text = format!("{}\n\n{text}", last.text);
            }
            _ => sections.push(Section { level, path, text }),
        }
    };
    push(0, vec![], lead);
    let mut stack: Vec<(u8, String)> = Vec::new();
    for (k, head) in heads.into_iter().enumerate() {
        if Some(k) == at_title {
            stack.clear();
            push(0, vec![], head.text);
            continue;
        }
        let level = head.level.max(1);
        while stack.last().is_some_and(|(l, _)| *l >= level) {
            stack.pop();
        }
        stack.push((level, head.title));
        push(
            level,
            stack.iter().map(|(_, h)| h.clone()).collect(),
            head.text,
        );
    }
    (title, sections)
}

/// An HTML document split at its h1 to h6 headings. Math keeps its TeX (`alttext`)
/// rather than the text of its MathML, and table cells are separated with |.
pub fn html_sections(html: &str) -> (Option<String>, Vec<Section>) {
    let mut html = html.to_owned();
    for tag in [
        "script", "style", "noscript", "template", "nav", "footer", "button", "svg", "dialog",
        "form",
    ] {
        html = remove_blocks(&html, tag);
    }
    let html = math_tex(&html);
    let lower = html.to_ascii_lowercase();
    // Each heading: where its tag opens, where it closes, its level and its text.
    let mut found: Vec<(usize, usize, u8, String)> = Vec::new();
    let mut at = 0;
    while let Some(start) = lower[at..].find("<h").map(|i| at + i) {
        let bytes = lower.as_bytes();
        let level = bytes.get(start + 2).copied().filter(u8::is_ascii_digit);
        let after = bytes.get(start + 3).copied();
        let heading = level
            .filter(|l| (b'1'..=b'6').contains(l))
            .filter(|_| after.is_some_and(|c| c == b'>' || c.is_ascii_whitespace()))
            .and_then(|l| {
                let open = tag_end(&html, start)?;
                let close = format!("</h{}>", l as char);
                let end = lower[open..].find(&close).map(|i| open + i)?;
                Some((open, end, end + close.len(), l - b'0'))
            });
        match heading {
            Some((open, end, after, level)) => {
                let title = one_line(&html_text(&html[open..end]));
                if !title.is_empty() {
                    found.push((start, after, level, title));
                }
                at = after;
            }
            None => at = start + 2,
        }
    }
    let lead = section_text(&html[..found.first().map_or(html.len(), |h| h.0)]);
    let heads = found
        .iter()
        .enumerate()
        .map(|(k, (_, after, level, title))| Heading {
            level: *level,
            title: title.clone(),
            text: section_text(&html[*after..found.get(k + 1).map_or(html.len(), |h| h.0)]),
        })
        .collect();
    nest(lead, heads, true)
}

/// A Markdown document split at its `#` headings, outside fenced code.
pub fn markdown_sections(text: &str) -> (Option<String>, Vec<Section>) {
    let mut lead = String::new();
    let mut heads: Vec<Heading> = Vec::new();
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        match fence {
            Some(f) if trimmed.starts_with(f) => fence = None,
            Some(_) => {}
            None if trimmed.starts_with("```") => fence = Some("```"),
            None if trimmed.starts_with("~~~") => fence = Some("~~~"),
            None => {
                if let Some((level, title)) = atx_heading(line) {
                    heads.push(Heading {
                        level,
                        title,
                        text: String::new(),
                    });
                    continue;
                }
            }
        }
        let body = heads.last_mut().map_or(&mut lead, |h| &mut h.text);
        body.push_str(line);
        body.push('\n');
    }
    let tidy = |text: &str| paragraphs(text).join("\n\n");
    let heads = heads
        .into_iter()
        .map(|h| Heading {
            text: tidy(&h.text),
            ..h
        })
        .collect();
    nest(tidy(&lead), heads, true)
}

/// `## Title ##` as a level and a title.
fn atx_heading(line: &str) -> Option<(u8, String)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    let after = &rest[hashes..];
    if indent > 3
        || !(1..=6).contains(&hashes)
        || !(after.is_empty() || after.starts_with([' ', '\t']))
    {
        return None;
    }
    let title = after.trim().trim_end_matches('#').trim();
    (!title.is_empty()).then(|| (hashes as u8, title.to_owned()))
}

/// A Word document split at its headings. Paragraph styles are matched by the names
/// styles.xml gives them ("heading 1", "Title"), which stay English in every language,
/// or by an outline level.
fn docx_sections(document: &str, styles: &str) -> (Option<String>, Vec<Section>) {
    let level_of = |name: &str, outline: Option<&str>| -> Option<u8> {
        let name = name.to_ascii_lowercase();
        if name == "title" {
            return Some(0);
        }
        if let Some(n) = name
            .strip_prefix("heading ")
            .and_then(|n| n.parse::<u8>().ok())
        {
            return Some(n.clamp(1, 6));
        }
        outline
            .and_then(|o| o.parse::<u8>().ok())
            .filter(|o| *o < 9)
            .map(|o| (o + 1).min(6))
    };
    let mut levels = std::collections::HashMap::new();
    for style in styles.split("<w:style ").skip(1) {
        let style = style.split("</w:style>").next().unwrap_or("");
        let open = format!(" {}", &style[..style.find('>').unwrap_or(style.len())]);
        let Some(id) = attribute(&open, "w:styleId") else {
            continue;
        };
        let name = tag_value(style, "<w:name ").unwrap_or_default();
        if let Some(level) = level_of(name, tag_value(style, "<w:outlineLvl ")) {
            levels.insert(id.to_owned(), level);
        }
    }
    let mut lead = String::new();
    let mut heads: Vec<Heading> = Vec::new();
    for paragraph in document.split("</w:p>") {
        let text = docx_text(paragraph).trim().to_owned();
        if text.is_empty() {
            continue;
        }
        let level = tag_value(paragraph, "<w:pStyle ")
            .and_then(|id| levels.get(id).copied())
            .or_else(|| level_of("", tag_value(paragraph, "<w:outlineLvl ")));
        match level {
            Some(level) => heads.push(Heading {
                level,
                title: one_line(&text),
                text: String::new(),
            }),
            None => {
                let body = heads.last_mut().map_or(&mut lead, |h| &mut h.text);
                body.push_str(&text);
                body.push_str("\n\n");
            }
        }
    }
    nest(lead, heads, false)
}

/// The `w:val` of the first tag that starts with `open`, such as `<w:pStyle `.
fn tag_value<'a>(xml: &'a str, open: &str) -> Option<&'a str> {
    let start = xml.find(open)?;
    let end = xml[start..].find('>').map_or(xml.len(), |e| start + e);
    attribute(&xml[start..end], "w:val")
}

/// A PDF's bookmarks from `pdftohtml -xml`: each one's depth, title and page.
fn pdf_outline(xml: &str) -> Vec<(usize, String, i32)> {
    let Some(start) = xml.find("<outline>") else {
        return vec![];
    };
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut rest = &xml[start..];
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if rest.starts_with("<outline>") {
            depth += 1;
            rest = &rest["<outline>".len()..];
        } else if rest.starts_with("</outline>") {
            depth = depth.saturating_sub(1);
            rest = &rest["</outline>".len()..];
            if depth == 0 {
                break;
            }
        } else if rest.starts_with("<item") {
            let Some(close) = rest.find("</item>") else {
                break;
            };
            let item = &rest[..close];
            let tag_end = item.find('>').map_or(item.len(), |e| e + 1);
            let tag = &item[..tag_end];
            let title = one_line(&decode(&strip_tags(&item[tag_end..])));
            if let Some(page) = attribute(tag, "page").and_then(|p| p.parse().ok()) {
                if !title.is_empty() && depth > 0 {
                    items.push((depth, title, page));
                }
            }
            rest = &rest[close + "</item>".len()..];
        } else {
            rest = &rest[1..];
        }
    }
    items
}

/// PDF pages cut where each bookmark's title appears on its page; a bookmark whose
/// title isn't found there starts at the top of its page.
fn pdf_pages(pages: &[&str], outline: &[(usize, String, i32)]) -> Vec<Page> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut starts: Vec<(i32, String, String)> = Vec::new();
    for (depth, title, page) in outline {
        while stack.last().is_some_and(|(d, _)| d >= depth) {
            stack.pop();
        }
        stack.push((*depth, title.clone()));
        let path = stack
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" › ");
        starts.push((*page, title.clone(), path));
    }
    let mut current: Option<String> = None;
    let mut out = Vec::new();
    for (i, text) in pages.iter().enumerate() {
        let number = i as i32 + 1;
        let mut cut = 0;
        for (_, title, path) in starts.iter().filter(|(p, _, _)| *p == number) {
            let at = find_loose(&text[cut..], title).map_or(cut, |at| cut + at);
            if at > cut || (at == 0 && cut == 0) {
                out.push(Page {
                    number: Some(number),
                    section: current.clone(),
                    text: text[cut..at].to_owned(),
                });
            }
            cut = at;
            current = Some(path.clone());
        }
        out.push(Page {
            number: Some(number),
            section: current.clone(),
            text: text[cut..].to_owned(),
        });
    }
    out.retain(|p| p.text.chars().any(char::is_alphanumeric));
    out
}

/// Where `title`'s words appear in `text` in order, with any spacing between them.
fn find_loose(text: &str, title: &str) -> Option<usize> {
    let words: Vec<String> = title
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    let first = words.first()?;
    let lower = text.to_ascii_lowercase();
    lower
        .match_indices(first.as_str())
        .map(|(i, _)| i)
        .find(|&i| {
            let mut at = i + first.len();
            words[1..].iter().all(|word| {
                let rest = &lower[at..];
                let skipped = rest.len() - rest.trim_start().len();
                let found = rest[skipped..].starts_with(word.as_str());
                at += skipped + word.len();
                found && skipped > 0
            })
        })
}

fn section_text(html: &str) -> String {
    let html = replace_ci(&replace_ci(html, "</td>", " | "), "</th>", " | ");
    paragraphs(&html_text(&html)).join("\n\n")
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where the tag opening at `start` ends, just past its `>`, skipping quoted values.
fn tag_end(html: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (i, c) in html[start..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(start + i + 1),
            _ => {}
        }
    }
    None
}

/// An attribute's raw value in an opening tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let name = name.as_str();
    let mut at = 0;
    while let Some(i) = lower[at..].find(name).map(|i| at + i) {
        let before = lower[..i].chars().last();
        let rest = lower[i + name.len()..].trim_start();
        if before.is_some_and(char::is_whitespace) && rest.starts_with('=') {
            let value = tag[tag.len() - rest.len() + 1..].trim_start();
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            return value[1..].find(quote).map(|end| &value[1..1 + end]);
        }
        at = i + name.len();
    }
    None
}

/// Math as its TeX: `<math alttext="x^2">…</math>` becomes ` $x^2$ `.
fn math_tex(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    while let Some(start) = lower[at..].find("<math").map(|i| at + i) {
        let Some(open) = tag_end(html, start) else {
            break;
        };
        out.push_str(&html[at..start]);
        if let Some(tex) = attribute(&html[start..open], "alttext") {
            out.push_str(&format!(" ${}$ ", tex.trim()));
        }
        at = lower[open..]
            .find("</math>")
            .map_or(open, |i| open + i + "</math>".len());
    }
    out.push_str(&html[at..]);
    out
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
        // An entity ends within a few bytes; ';' is one byte, so this never splits a character.
        let end = rest.bytes().take(12).position(|b| b == b';');
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
    pub section: Option<String>,
    pub text: String,
}

/// Splits pages into passages of about PASSAGE characters along paragraph and sentence
/// boundaries. A passage never spans pages or sections, so a citation names one of each.
pub fn split(pages: &[Page]) -> Vec<Passage> {
    let mut passages = Vec::new();
    for page in pages {
        let paragraphs = paragraphs(&page.text);
        let before = passages.len();
        let mut current = String::new();
        let push = |current: &mut String, passages: &mut Vec<Passage>| {
            let text = current.trim().to_owned();
            if text.chars().filter(|c| c.is_alphanumeric()).count() >= 20 {
                passages.push(Passage {
                    page: page.number,
                    section: page.section.clone(),
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
        if current.len() > OVERLAP || passages.len() == before {
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
        assert_eq!(kind("notes.md", ""), Some(Kind::Markdown));
        assert_eq!(kind("report.docx", ""), Some(Kind::Docx));
        assert_eq!(kind("photo.jpg", "image/jpeg"), None);
    }

    #[test]
    fn web_pages_split_at_their_headings() {
        // The shape arXiv's HTML papers have.
        let html = r#"<html><head><style>h1{}</style></head><body><dialog><h2>Report an issue</h2><form><h3>Details</h3></form></dialog><nav class="ltx_TOC"><a>1 Introduction</a></nav>
<h1 class="ltx_title">Warmup <em>matters</em></h1><div class="ltx_authors">A. Author</div>
<div class="ltx_abstract"><h6>Abstract</h6><p>We study warmup.</p></div>
<section><h2><span class="ltx_tag">1 </span>Introduction</h2><p>Training is hard.</p></section>
<section><h2>2 Method</h2><h3 class="x" data-note="a>b">2.1 Schedule</h3><p>We use <math alttext="\eta_t = t/T" display="inline"><mi>η</mi><mo>=</mo></math> for 500 steps.</p>
<table><tr><th>lr</th><th>loss</th></tr><tr><td>3e-4</td><td>2.41</td></tr></table>
<h3>2.2 Data</h3><p>Deduplicated &amp; filtered.</p></section>
<h2>References</h2><p>[1] Someone.</p><footer>arXiv footer</footer></body></html>"#;
        let (title, sections) = html_sections(html);
        assert_eq!(title.as_deref(), Some("Warmup matters"));
        let paths: Vec<String> = sections.iter().map(|s| s.path.join(" › ")).collect();
        assert_eq!(
            paths,
            [
                "",
                "Abstract",
                "1 Introduction",
                "2 Method",
                "2 Method › 2.1 Schedule",
                "2 Method › 2.2 Data",
                "References"
            ]
        );
        // The text under the title, before the first heading, is the lead.
        assert_eq!(
            (sections[0].level, sections[0].text.as_str()),
            (0, "A. Author")
        );
        assert_eq!(sections[1].level, 6);
        assert_eq!(sections[1].text, "We study warmup.");
        assert_eq!(sections[3].text, "");
        assert_eq!(
            sections[4].text,
            "We use $\\eta_t = t/T$ for 500 steps.\n\nlr | loss |\n\n3e-4 | 2.41 |"
        );
        assert_eq!(sections[5].text, "Deduplicated & filtered.");
        assert!(!sections.iter().any(|s| s.text.contains("footer")
            || s.text.contains("1 Introduction")
            || s.text.contains("Report")));
        // Without a title, text before the first heading is the lead; with several h1s,
        // each heads its own path.
        let (title, sections) = html_sections(
            "<p>Intro.</p><h1>One</h1><p>A.</p><h2>Sub</h2><p>B.</p><h1>Two</h1><p>C.</p>",
        );
        assert_eq!(title, None);
        let paths: Vec<String> = sections.iter().map(|s| s.path.join(" › ")).collect();
        assert_eq!(paths, ["", "One", "One › Sub", "Two"]);
        assert_eq!(sections[0].text, "Intro.");
        // A page without headings is all lead.
        assert_eq!(
            html_sections("<p>short</p>").1[0].path,
            Vec::<String>::new()
        );
    }

    #[test]
    fn markdown_splits_at_its_headings_outside_code() {
        let text = "Intro line.\n\n# Guide\n\nWhat this is.\n\n## Install ##\n\nRun:\n\n```sh\n# not a heading\npip install x\n```\n\n### On a cluster\n\nLoad the module first.\n\n## Use\n\n    # indented code, not a heading\n#hashtag is text\n";
        let (title, sections) = markdown_sections(text);
        assert_eq!(title.as_deref(), Some("Guide"));
        let paths: Vec<String> = sections.iter().map(|s| s.path.join(" › ")).collect();
        assert_eq!(paths, ["", "Install", "Install › On a cluster", "Use"]);
        assert_eq!(sections[0].text, "Intro line.\n\nWhat this is.");
        assert!(sections[1].text.contains("# not a heading pip install x"));
        assert!(sections[3].text.contains("#hashtag is text"));
        assert_eq!(atx_heading("####### seven"), None);
        assert_eq!(atx_heading("#"), None);
    }

    #[test]
    fn word_headings_come_from_style_names() {
        // A German document: the style ids are translated, the names are not.
        let styles = r#"<w:styles><w:style w:type="paragraph" w:styleId="Titel"><w:name w:val="Title"/></w:style><w:style w:type="paragraph" w:styleId="berschrift1"><w:name w:val="heading 1"/></w:style><w:style w:type="paragraph" w:styleId="berschrift2"><w:name w:val="heading 2"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Standard"><w:name w:val="Normal"/></w:style></w:styles>"#;
        let p = |style: &str, text: &str| {
            format!(
                r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
            )
        };
        let document = [
            p("Titel", "Handbuch"),
            p("Standard", "Einleitung."),
            p("berschrift1", "Installation"),
            p("Standard", "Erst &amp; dann."),
            p("berschrift2", "Auf dem Cluster"),
            p("Standard", "Modul laden."),
            r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>Anhang</w:t></w:r></w:p>"#
                .into(),
            p("Standard", "Mehr."),
        ]
        .concat();
        let (title, sections) = docx_sections(&document, styles);
        assert_eq!(title.as_deref(), Some("Handbuch"));
        let found: Vec<(String, String)> = sections
            .iter()
            .map(|s| (s.path.join(" › "), s.text.trim().to_owned()))
            .collect();
        assert_eq!(
            found,
            [
                ("".into(), "Einleitung.".into()),
                ("Installation".into(), "Erst & dann.".into()),
                (
                    "Installation › Auf dem Cluster".into(),
                    "Modul laden.".into()
                ),
                ("Anhang".into(), "Mehr.".into()),
            ]
        );
        // Without styles.xml, only outline levels make headings.
        assert_eq!(docx_sections(&document, "").1.len(), 2);
    }

    #[test]
    fn pdf_bookmarks_cut_pages_into_sections() {
        let xml = r#"<?xml version="1.0"?><pdf2xml><page number="1"><text>&lt;outline&gt;</text></page>
<outline>
<item page="1">1 Introduction</item>
<item page="2">2 Method</item>
<outline>
<item page="2">2.1 Schedule &amp; warmup</item>
<item>No page</item>
</outline>
<item page="3">3 Results</item>
</outline>
</pdf2xml>"#;
        let outline = pdf_outline(xml);
        assert_eq!(
            outline,
            [
                (1, "1 Introduction".into(), 1),
                (1, "2 Method".into(), 2),
                (2, "2.1 Schedule & warmup".into(), 2),
                (1, "3 Results".into(), 3),
            ]
        );
        let pages = [
            "Title page\n1 Introduction\nWhy warmup.",
            "More intro.\n2 Method\nWe train.\n2.1 Schedule &\nwarmup\nLinear for 500 steps.",
            "Results first.\n3.  Results\nLoss 2.31.",
        ];
        let parts: Vec<(Option<i32>, Option<String>, String)> = pdf_pages(&pages, &outline)
            .into_iter()
            .map(|p| (p.number, p.section, p.text.trim().to_owned()))
            .collect();
        let s = |t: &str| Some(t.to_owned());
        assert_eq!(
            parts,
            [
                (Some(1), None, "Title page".into()),
                (
                    Some(1),
                    s("1 Introduction"),
                    "1 Introduction\nWhy warmup.".into()
                ),
                (Some(2), s("1 Introduction"), "More intro.".into()),
                (Some(2), s("2 Method"), "2 Method\nWe train.".into()),
                (
                    Some(2),
                    s("2 Method › 2.1 Schedule & warmup"),
                    "2.1 Schedule &\nwarmup\nLinear for 500 steps.".into()
                ),
                // "3.  Results" isn't "3 Results": the section starts at the top of its page.
                (
                    Some(3),
                    s("3 Results"),
                    "Results first.\n3.  Results\nLoss 2.31.".into()
                ),
            ]
        );
        assert_eq!(pdf_outline("<pdf2xml></pdf2xml>"), []);
    }

    #[test]
    fn short_sections_keep_their_own_passages() {
        let part = |section: &str, text: &str| Page {
            number: None,
            section: Some(section.into()),
            text: text.into(),
        };
        let passages = split(&[
            part("Install", "Load the module, then install the package."),
            part("Use", "Run the solver with the config file you made."),
        ]);
        let sections: Vec<_> = passages.iter().map(|p| p.section.as_deref()).collect();
        assert_eq!(sections, [Some("Install"), Some("Use")]);
    }

    #[test]
    fn an_ampersand_before_wide_characters_is_kept() {
        assert_eq!(
            decode("R&D team 用户手册 &amp; 指南"),
            "R&D team 用户手册 & 指南"
        );
        assert_eq!(decode("&用户"), "&用户");
        assert_eq!(html_text("<p>Q&A：用户</p>").trim(), "Q&A：用户");
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

    /// An Office file whose part `name` inflates from about half a megabyte to past the
    /// output cap: fixed-Huffman deflate of one zero byte followed by 258-byte copies.
    fn zip_bomb(name: &str) -> Vec<u8> {
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
        let name = name.as_bytes();
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
        let bomb = zip_bomb("word/document.xml");
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

    #[tokio::test]
    async fn slides_and_sheets_that_expand_past_the_cap_fail_cleanly() {
        let tools = Tools::from_env();
        if !unzip_available(&tools) {
            return;
        }
        for (kind, part, says) in [
            (
                Kind::Pptx,
                "ppt/presentation.xml",
                "this PowerPoint file could not be opened",
            ),
            (
                Kind::Xlsx,
                "xl/workbook.xml",
                "this Excel file could not be opened",
            ),
        ] {
            let bomb = zip_bomb(part);
            let error =
                match tokio::time::timeout(Duration::from_secs(60), extract(&tools, kind, &bomb))
                    .await
                    .expect("extraction was not stopped")
                {
                    Ok(_) => panic!("a file past the cap was extracted"),
                    Err(error) => format!("{error:#}"),
                };
            assert!(
                error.contains(says) && error.contains("more text than Chat can index"),
                "{error}"
            );
        }
    }

    fn unzip_available(tools: &Tools) -> bool {
        let found = std::process::Command::new(&tools.unzip)
            .arg("-v")
            .output()
            .is_ok();
        if !found {
            eprintln!("skipping: {} is not available", tools.unzip);
        }
        found
    }

    /// A zip archive holding each part as it is, uncompressed.
    fn zip(parts: &[(&str, &str)]) -> Vec<u8> {
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
        let crc = |bytes: &[u8]| {
            !bytes.iter().fold(!0u32, |c, b| {
                table[((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8)
            })
        };
        let header = |out: &mut Vec<u8>, name: &str, data: &[u8], central: Option<u32>| {
            out.extend(
                if central.is_some() {
                    0x0201_4b50u32
                } else {
                    0x0403_4b50
                }
                .to_le_bytes(),
            );
            if central.is_some() {
                out.extend(20u16.to_le_bytes());
            }
            out.extend(20u16.to_le_bytes());
            out.extend(0u16.to_le_bytes()); // flags
            out.extend(0u16.to_le_bytes()); // stored
            out.extend(0u32.to_le_bytes()); // time and date
            out.extend(crc(data).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(0u16.to_le_bytes()); // extra
            if let Some(offset) = central {
                out.extend([0u8; 10]); // comment, disk, attributes
                out.extend(offset.to_le_bytes());
            }
            out.extend(name.as_bytes());
        };
        let mut out = Vec::new();
        let mut offsets = Vec::new();
        for (name, data) in parts {
            offsets.push(out.len() as u32);
            header(&mut out, name, data.as_bytes(), None);
            out.extend(data.as_bytes());
        }
        let directory = out.len() as u32;
        for ((name, data), offset) in parts.iter().zip(offsets) {
            header(&mut out, name, data.as_bytes(), Some(offset));
        }
        let size = out.len() as u32 - directory;
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend((parts.len() as u16).to_le_bytes());
        out.extend((parts.len() as u16).to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(directory.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    fn sections(pages: &[Page]) -> Vec<(String, String)> {
        pages
            .iter()
            .map(|p| (p.section.clone().unwrap_or_default(), p.text.clone()))
            .collect()
    }

    /// A PowerPoint file built here: two slides stored out of order, a title, a table
    /// and speaker notes.
    fn pptx() -> Vec<u8> {
        let shape = |kind: Option<&str>, text: &str| {
            let ph = kind.map_or(String::new(), |k| {
                format!(r#"<p:nvPr><p:ph type="{k}"/></p:nvPr>"#)
            });
            format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Shape"/>{ph}</p:nvSpPr><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#
            )
        };
        let cell = |text: &str| {
            format!(r#"<a:tc><a:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></a:txBody></a:tc>"#)
        };
        let table = format!(
            r#"<p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tr>{}{}</a:tr><a:tr>{}{}</a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
            cell("Region"),
            cell("Sales"),
            cell("North"),
            cell("12")
        );
        let slide = |body: String| {
            format!(
                r#"<?xml version="1.0"?><p:sld><p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sld>"#
            )
        };
        let first = slide(format!(
            "{}{}{table}",
            shape(Some("ctrTitle"), "Quarterly results"),
            r#"<p:sp><p:txBody><a:p><a:r><a:t>Revenue grew &amp; costs fell</a:t></a:r><a:br/><a:r><a:t>in every region</a:t></a:r></a:p></p:txBody></p:sp>"#
        ));
        let second = slide(shape(None, "Thanks for listening"));
        let notes = format!(
            r#"<p:notes><p:cSld><p:spTree>{}{}{}</p:spTree></p:cSld></p:notes>"#,
            shape(Some("sldImg"), ""),
            shape(Some("body"), "Mention the new region."),
            shape(Some("sldNum"), "1")
        );
        zip(&[
            ("[Content_Types].xml", "<Types/>"),
            (
                "ppt/presentation.xml",
                r#"<p:presentation><p:sldIdLst><p:sldId id="257" r:id="rId3"/><p:sldId id="256" r:id="rId2"/></p:sldIdLst></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/></Relationships>"#,
            ),
            ("ppt/slides/slide1.xml", &second),
            ("ppt/slides/slide2.xml", &first),
            (
                "ppt/slides/_rels/slide2.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide1.xml"/></Relationships>"#,
            ),
            ("ppt/notesSlides/notesSlide1.xml", &notes),
        ])
    }

    /// An Excel file built here: shared, rich, inline and formula cells, a gap, a
    /// boolean, and a second sheet named by an absolute path.
    fn xlsx() -> Vec<u8> {
        zip(&[
            (
                "xl/workbook.xml",
                r#"<workbook><sheets><sheet name="Budget" sheetId="1" r:id="rId1"/><sheet name="Notes &amp; misc" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="worksheet" Target="/xl/worksheets/sheet2.xml"/><Relationship Id="rId3" Type="sharedStrings" Target="sharedStrings.xml"/></Relationships>"#,
            ),
            (
                "xl/sharedStrings.xml",
                r#"<sst count="3"><si><t>Item</t></si><si><t>Cost</t></si><si><r><t xml:space="preserve">Rent, </t></r><r><t>office</t></r><rPh sb="0" eb="1"><t>RENTO</t></rPh></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><cols><col min="1" max="1"/></cols><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2"><v>1200</v></c></row><row r="3"/><row r="4"><c r="A4" t="inlineStr"><is><t>Total</t></is></c><c r="C4"><f>SUM(B2:B2)</f><v>1200</v></c></row><row r="5"><c r="A5" t="b"><v>1</v></c></row></sheetData></worksheet>"#,
            ),
            (
                "xl/worksheets/sheet2.xml",
                r#"<worksheet><sheetData><row r="1"><c r="B1" t="str"><v>He said "hi"</v></c></row></sheetData></worksheet>"#,
            ),
        ])
    }

    #[tokio::test]
    async fn slides_are_read_in_order_with_their_notes() {
        let tools = Tools::from_env();
        if !unzip_available(&tools) {
            return;
        }
        let Extracted::Pages(pages) = extract(&tools, Kind::Pptx, &pptx()).await.unwrap() else {
            panic!("slides need no text recognition");
        };
        assert_eq!(
            sections(&pages),
            [
                (
                    "Slide 1: Quarterly results".into(),
                    "Revenue grew & costs fell\nin every region\nRegion | Sales |\nNorth | 12 |\n\nSpeaker notes:\nMention the new region.".into()
                ),
                ("Slide 2".into(), "Thanks for listening".into()),
            ]
        );
        assert_eq!(parts(Kind::Pptx, &pages), Some(2));
        let marked = marked_text(&pages);
        assert!(
            marked.starts_with("[Slide 1: Quarterly results]\nRevenue grew")
                && marked.contains("\n\n[Slide 2]\nThanks for listening"),
            "{marked}"
        );
    }

    #[tokio::test]
    async fn sheets_become_comma_separated_rows() {
        let tools = Tools::from_env();
        if !unzip_available(&tools) {
            return;
        }
        let Extracted::Pages(pages) = extract(&tools, Kind::Xlsx, &xlsx()).await.unwrap() else {
            panic!("sheets need no text recognition");
        };
        assert_eq!(
            sections(&pages),
            [
                (
                    "Sheet: Budget".into(),
                    "Item,Cost\n\"Rent, office\",1200\nTotal,,1200\nTRUE".into()
                ),
                (
                    "Sheet: Notes & misc".into(),
                    ",\"He said \"\"hi\"\"\"".into()
                ),
            ]
        );
        assert_eq!(parts(Kind::Xlsx, &pages), Some(2));
    }

    #[test]
    fn a_long_sheet_says_where_it_was_cut() {
        let row = |n: usize| {
            format!(r#"<row r="{n}"><c r="A{n}"><v>{n}</v></c><c r="B{n}"><v>x</v></c></row>"#)
        };
        let sheet: String = (1..=4).map(row).collect();
        let mut left = 3;
        let text = sheet_text(&sheet, &[], &mut left);
        assert_eq!(
            text,
            "1,x\n2,x\n[Cut here: 2 more rows are not included. Chat reads up to 100,000 cells of a workbook.]"
        );
        // A stray cell far to the right doesn't pad the row out to it.
        let mut left = 10;
        assert_eq!(
            sheet_text(
                r#"<row><c r="A1"><v>a</v></c><c r="ZZ1"><v>far</v></c></row>"#,
                &[],
                &mut left
            ),
            "a"
        );
        assert_eq!(column_index("AB7"), Some(27));
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn office_parts_are_plain_paths() {
        assert_eq!(
            resolve("ppt/slides", "../notesSlides/n1.xml"),
            "ppt/notesSlides/n1.xml"
        );
        assert_eq!(
            resolve("xl", "/xl/worksheets/sheet2.xml"),
            "xl/worksheets/sheet2.xml"
        );
        assert_eq!(
            rels_of("ppt/slides/slide3.xml").1,
            "ppt/slides/_rels/slide3.xml.rels"
        );
    }

    #[tokio::test]
    async fn parts_named_with_patterns_are_not_read() {
        let tools = Tools::from_env();
        if !unzip_available(&tools) {
            return;
        }
        let mut zip = Zip::new(&tools.unzip, &pptx()).await.unwrap();
        for part in ["ppt/slides/*", "ppt/../x.xml", "", "ppt/[a].xml"] {
            assert!(zip.read(part).await.is_err(), "{part}");
        }
        assert!(zip.read("ppt/presentation.xml").await.is_ok());
    }

    #[test]
    fn marked_text_names_pages_and_sections_once() {
        let page = |number: Option<i32>, section: Option<&str>, text: &str| Page {
            number,
            section: section.map(Into::into),
            text: text.into(),
        };
        let text = marked_text(&[
            page(Some(1), None, "Title page"),
            page(Some(1), Some("1 Intro"), "Why."),
            page(Some(2), Some("1 Intro"), "More."),
            page(None, None, "  "),
        ]);
        assert_eq!(
            text,
            "[Page 1]\nTitle page\n\n[Page 1 · 1 Intro]\nWhy.\n\n[Page 2 · 1 Intro]\nMore."
        );
    }

    #[test]
    fn passages_stay_on_their_page_and_overlap() {
        let long = "Entropy is a measure of the number of microstates. ".repeat(80);
        let pages = vec![
            Page {
                number: Some(1),
                section: None,
                text: format!("Intro paragraph about heat engines and work.\n\n{long}"),
            },
            Page {
                number: Some(2),
                section: None,
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
