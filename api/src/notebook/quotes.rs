//! Quotations in what the Notebook model makes from a notebook's sources. The model
//! writes each one as [[S2 "the exact words"]] after the statement it supports; every
//! one is looked up in the named source's text here, in code, before it is shown. A
//! quotation found there becomes a numbered citation linked to its document and page;
//! one that is not is marked unverified and never shown as a citation.

use serde_json::{json, Value};
use uuid::Uuid;

/// One source as the model read it: its passages in order, without their overlaps.
pub struct Source {
    pub document: Uuid,
    pub name: String,
    /// (page, section, text) of each passage.
    pub passages: Vec<(Option<i32>, Option<String>, String)>,
}

/// A source's text normalised for matching, with where each passage starts in it.
pub struct Index {
    text: String,
    starts: Vec<(usize, Option<i32>, Option<String>)>,
}

impl Index {
    pub fn new(source: &Source) -> Self {
        let mut text = String::new();
        let mut starts = Vec::with_capacity(source.passages.len());
        let mut previous: Option<&(Option<i32>, Option<String>, String)> = None;
        for passage in &source.passages {
            let content = match previous {
                Some(prev) if prev.0 == passage.0 && prev.1 == passage.1 => {
                    super::guides::without_overlap(&prev.2, &passage.2)
                }
                _ => passage.2.as_str(),
            };
            if !text.is_empty() {
                text.push(' ');
            }
            starts.push((text.len(), passage.0, passage.1.clone()));
            text.push_str(&normalize(content));
            previous = Some(passage);
        }
        Self { text, starts }
    }

    /// Where the quotation is: the page and section of the passage it starts in. Parts
    /// of a quotation cut with an ellipsis must appear in order.
    pub fn find(&self, quotation: &str) -> Option<(Option<i32>, Option<String>)> {
        let parts: Vec<String> = quotation
            .split(['…'])
            .flat_map(|p| p.split("..."))
            .map(normalize)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.iter().map(|p| p.chars().count()).sum::<usize>() < MIN_QUOTE {
            return None;
        }
        let mut from = 0;
        let mut first = None;
        for part in &parts {
            let at = from + self.text.get(from..)?.find(part.as_str())?;
            first.get_or_insert(at);
            from = at + part.len();
        }
        let at = first?;
        let (_, page, section) = self
            .starts
            .iter()
            .rev()
            .find(|(start, _, _)| *start <= at)?
            .clone();
        Some((page, section))
    }
}

/// The fewest characters a quotation must have to count: shorter ones match anywhere.
const MIN_QUOTE: usize = 12;

/// Text as it is compared: lower case, one kind of quote mark, apostrophe and dash,
/// no soft hyphens, common ligatures spelled out, and single spaces.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        let mapped: &str = match c {
            '‘' | '’' | '‚' | '‛' | '′' | '`' | '´' => "'",
            '“' | '”' | '„' | '‟' | '″' | '«' | '»' => "\"",
            '‐' | '‑' | '‒' | '–' | '—' | '―' | '−' => "-",
            '\u{00AD}' | '\u{200B}' | '\u{FEFF}' => "",
            'ﬁ' => "fi",
            'ﬂ' => "fl",
            'ﬀ' => "ff",
            'ﬃ' => "ffi",
            'ﬄ' => "ffl",
            c if c.is_whitespace() => {
                space = !out.is_empty();
                continue;
            }
            _ => {
                if space {
                    out.push(' ');
                    space = false;
                }
                out.extend(c.to_lowercase());
                continue;
            }
        };
        if mapped.is_empty() {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push_str(mapped);
    }
    out
}

/// A quotation as the model wrote it.
#[derive(Debug, PartialEq)]
struct Marked<'a> {
    whole: std::ops::Range<usize>,
    source: Option<usize>,
    quotation: &'a str,
}

/// Every [[S<n> "words"]] in the text, in order. `source` is the zero-based index of
/// S<n>, None when the id is not a number.
fn marked(text: &str) -> Vec<Marked<'_>> {
    let mut out = vec![];
    let mut from = 0;
    while let Some(open) = text.get(from..).and_then(|t| t.find("[[")) {
        let start = from + open;
        let Some(close) = text[start..].find("]]") else {
            break;
        };
        let end = start + close + 2;
        let inner = text[start + 2..end - 2].trim();
        from = end;
        let Some(rest) = inner.strip_prefix(['S', 's']) else {
            continue;
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let rest = rest[digits.len()..].trim_start_matches([':', ',', ' ', '\u{00A0}']);
        let quotation = rest
            .trim()
            .trim_start_matches(['"', '“', '”', '\''])
            .trim_end_matches(['"', '“', '”', '\''])
            .trim();
        if digits.is_empty() || quotation.is_empty() {
            continue;
        }
        out.push(Marked {
            whole: start..end,
            source: digits.parse::<usize>().ok().and_then(|n| n.checked_sub(1)),
            quotation,
        });
    }
    out
}

/// What checking a made text's quotations came to.
#[derive(Debug, PartialEq)]
pub struct Checked {
    /// The text with each verified quotation as [n] and each other as "[unverified]",
    /// and those listed at its end.
    pub body: String,
    /// Each citation: n, documentId, name, page, section and the quotation.
    pub citations: Vec<Value>,
    /// Quotations not found in the source they named.
    pub unverified: Vec<Value>,
}

/// Checks every quotation in `text` against the sources' text.
pub fn check(text: &str, sources: &[Source]) -> Checked {
    let indexes: Vec<Index> = sources.iter().map(Index::new).collect();
    let mut body = String::with_capacity(text.len());
    let mut citations: Vec<Value> = vec![];
    let mut unverified: Vec<Value> = vec![];
    let mut last = 0;
    for mark in marked(text) {
        // A space before a citation that follows the sentence directly is kept as written.
        body.push_str(&text[last..mark.whole.start]);
        last = mark.whole.end;
        let quotation: String = mark.quotation.chars().take(600).collect();
        let found = mark
            .source
            .and_then(|i| Some((i, indexes.get(i)?.find(&quotation)?)));
        match found {
            Some((i, (page, section))) => {
                let source = &sources[i];
                // The same words from the same place keep their number.
                let n = match citations.iter().position(|c| {
                    c["documentId"] == json!(source.document) && c["quote"] == json!(quotation)
                }) {
                    Some(at) => at + 1,
                    None => {
                        citations.push(json!({"n":citations.len() + 1,"documentId":source.document,"name":source.name,
                            "page":page,"section":section,"quote":quotation}));
                        citations.len()
                    }
                };
                body.push_str(&format!("[{n}]"));
            }
            None => {
                body.push_str("[unverified]");
                let name = mark
                    .source
                    .and_then(|i| sources.get(i))
                    .map(|s| s.name.clone());
                unverified.push(json!({"name":name,"quote":quotation}));
            }
        }
    }
    body.push_str(&text[last..]);
    let mut body = body.trim().to_owned();
    if !unverified.is_empty() {
        body.push_str("\n\n---\n\n**Quotations not found in the sources.** These were not shown as citations, because their words could not be found in the source they named:\n");
        for q in &unverified {
            let quote = q["quote"].as_str().unwrap_or("").replace(['\n', '\r'], " ");
            match q["name"].as_str() {
                Some(name) => body.push_str(&format!("\n- “{quote}” ({name})")),
                None => body.push_str(&format!("\n- “{quote}” (no such source)")),
            }
        }
    }
    Checked {
        body,
        citations,
        unverified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(passages: &[(i32, &str)]) -> Source {
        Source {
            document: Uuid::from_u128(1),
            name: "Serving stack.pdf".into(),
            passages: passages
                .iter()
                .map(|(page, text)| (Some(*page), Some("4 Results".to_owned()), text.to_string()))
                .collect(),
        }
    }

    #[test]
    fn a_quotation_is_found_in_its_source_whatever_its_spacing_quotes_and_case() {
        let s = source(&[
            (9, "Setup.\n\nWe ran three models."),
            (
                10,
                "Throughput is flat\n beyond 320 concurrent sequences on all three models, while the server’s latency keeps rising — sharply.",
            ),
        ]);
        let index = Index::new(&s);
        let page = |q: &str| index.find(q).map(|(page, _)| page);
        assert_eq!(
            page("throughput is flat beyond 320 concurrent sequences"),
            Some(Some(10))
        );
        assert_eq!(
            page("the server's latency keeps rising - sharply"),
            Some(Some(10))
        );
        assert_eq!(page("THROUGHPUT  IS  FLAT"), Some(Some(10)));
        assert_eq!(
            page("Throughput is flat … latency keeps rising"),
            Some(Some(10)),
            "an ellipsis joins parts found in order"
        );
        assert_eq!(
            page("latency keeps rising ... throughput is flat"),
            None,
            "out of order"
        );
        assert_eq!(page("throughput is flat beyond 512 sequences"), None);
        assert_eq!(page("We ran"), None, "too short to mean anything");
        assert_eq!(page("We ran three models"), Some(Some(9)));
    }

    #[test]
    fn verified_quotations_become_citations_and_others_never_do() {
        let a = source(&[(10, "Throughput is flat beyond 320 concurrent sequences.")]);
        let mut b = source(&[(2, "Gains level off past 96 sequences on GPUs.")]);
        b.document = Uuid::from_u128(2);
        b.name = "Haddad.pdf".into();
        let text = "# Briefing\n\nThroughput flattens past 320 [[S1 \"Throughput is flat beyond 320 concurrent sequences\"]]. GPUs level off earlier [[S2: “gains level off past 96 sequences”]], and again [[S1 \"throughput is flat beyond 320 concurrent sequences\"]]. A claim [[S1 \"throughput doubles past 512 sequences\"]]. Misnamed [[S1 \"Gains level off past 96 sequences\"]]. Unknown [[S7 \"Throughput is flat beyond 320\"]]. Not a quote [[link]].";
        let checked = check(text, &[a, b]);
        assert_eq!(checked.citations.len(), 3);
        assert_eq!(checked.citations[0]["page"], 10);
        assert_eq!(checked.citations[1]["name"], "Haddad.pdf");
        assert_eq!(checked.citations[1]["page"], 2);
        assert!(
            checked.body.starts_with("# Briefing\n\nThroughput flattens past 320 [1]. GPUs level off earlier [2], and again [3]. A claim [unverified]. Misnamed [unverified]. Unknown [unverified]. Not a quote [[link]]."),
            "{}",
            checked.body
        );
        assert_eq!(checked.unverified.len(), 3);
        assert!(checked
            .body
            .contains("- “throughput doubles past 512 sequences” (Serving stack.pdf)"));
        assert!(checked.body.contains("(no such source)"));
        // Quotations of the same words from the same source share their number.
        let twice = check(
            "A [[S1 \"Throughput is flat beyond 320 concurrent sequences\"]] B [[S1 \"Throughput is flat beyond 320 concurrent sequences\"]]",
            &[source(&[(10, "Throughput is flat beyond 320 concurrent sequences.")])],
        );
        assert_eq!(
            (twice.body.as_str(), twice.citations.len()),
            ("A [1] B [1]", 1)
        );
    }

    #[test]
    fn overlapping_passages_are_read_once() {
        let s = source(&[
            (3, "First part of the text.\n\nThe tail carries over"),
            (3, "The tail carries over\n\ninto the next passage here."),
        ]);
        let index = Index::new(&s);
        assert_eq!(
            index.find("the tail carries over into the next passage"),
            Some((Some(3), Some("4 Results".into())))
        );
    }
}
