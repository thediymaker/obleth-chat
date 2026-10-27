//! read_paper: a paper, or any web page with headings, read by section. The page is
//! fetched in the workspace, as the person's own commands would fetch it, and kept
//! there, so reading more of it later costs nothing. When the chat says what it is
//! looking for, one pick-one decision over the outline ranks the sections, as in
//! table-of-contents retrieval, and the likeliest come back in full.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::{
    decisions::{choice, Asker, Settings},
    knowledge::extract::{html_sections, Section},
};

/// Options one pick-one question may have.
const MAX_OPTIONS: usize = 255;
/// Sections returned for what the chat is looking for, and the least likely one kept
/// after the first.
const PICKS: usize = 3;
const FLOOR: f64 = 0.05;
/// Characters returned of each section, with what is under it.
const SECTION_TEXT: usize = 6 * 1024;
/// Characters of a section's start shown with its heading when asking.
const PREVIEW: usize = 160;
const OUTLINE_LINES: usize = 150;
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Paper {
    pub title: Option<String>,
    pub sections: Vec<Section>,
}

impl Paper {
    pub fn from_html(html: &str) -> Self {
        let (title, sections) = html_sections(html);
        Self { title, sections }
    }

    /// A section by its id from the outline ("s4"), its number ("3.2"), or its heading.
    pub fn find(&self, key: &str) -> Option<usize> {
        let key = key.trim().trim_start_matches('§').trim().to_lowercase();
        if let Some(n) = key.strip_prefix('s').and_then(|n| n.parse::<usize>().ok()) {
            return (1..=self.sections.len()).contains(&n).then(|| n - 1);
        }
        let heading = |s: &Section| s.path.last().map(|h| h.to_lowercase()).unwrap_or_default();
        self.sections
            .iter()
            .position(|s| {
                let h = heading(s);
                h == key || h.starts_with(&format!("{key} "))
            })
            .or_else(|| {
                (!key.is_empty())
                    .then(|| self.sections.iter().position(|s| heading(s).contains(&key)))
                    .flatten()
            })
    }

    /// A section's text with the sections under it, cut at SECTION_TEXT.
    pub fn body(&self, i: usize) -> String {
        let depth = self.sections[i].path.len();
        let mut out = self.sections[i].text.clone();
        for s in self.sections[i + 1..]
            .iter()
            .take_while(|s| s.path.len() > depth)
        {
            out.push_str(&format!(
                "\n\n#### {}\n{}",
                s.path.last().map_or("", String::as_str),
                s.text
            ));
            if out.len() > SECTION_TEXT {
                break;
            }
        }
        cut(out.trim(), SECTION_TEXT)
    }

    /// Every section's id and heading, indented by depth.
    pub fn outline(&self) -> String {
        let mut lines: Vec<String> = self
            .sections
            .iter()
            .enumerate()
            .take(OUTLINE_LINES)
            .map(|(i, s)| {
                format!(
                    "{} {}{}",
                    id(i),
                    "  ".repeat(s.path.len().saturating_sub(1)),
                    s.path.last().map_or("", String::as_str)
                )
            })
            .collect();
        if self.sections.len() > OUTLINE_LINES {
            lines.push(format!(
                "… and {} more",
                self.sections.len() - OUTLINE_LINES
            ));
        }
        lines.join("\n")
    }

    /// A section's place: its heading path.
    pub fn heading(&self, i: usize) -> String {
        self.sections[i].path.join(" › ")
    }

    /// The section a reader starts with: the abstract, or else the first.
    pub fn start(&self) -> usize {
        self.sections
            .iter()
            .position(|s| {
                s.path
                    .last()
                    .is_some_and(|h| h.eq_ignore_ascii_case("abstract"))
            })
            .unwrap_or(0)
    }

    /// Each section from `top` down to the next one at its depth or above.
    fn under(&self, top: usize) -> impl Iterator<Item = usize> + '_ {
        let depth = self.sections[top].path.len();
        std::iter::once(top).chain(
            (top + 1..self.sections.len())
                .take_while(move |&i| self.sections[i].path.len() > depth),
        )
    }
}

pub fn id(i: usize) -> String {
    format!("s{}", i + 1)
}

fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", text[..end].trim_end())
}

/// The page to fetch for a link. arXiv's abstract and PDF links become its HTML version.
pub fn source(link: &str) -> Result<String, String> {
    let wrong = || {
        "Give the paper as an http(s) link, such as https://arxiv.org/abs/2609.03874".to_string()
    };
    let url = url::Url::parse(link.trim()).map_err(|_| wrong())?;
    let host = url.host_str().ok_or_else(wrong)?.to_lowercase();
    if !matches!(url.scheme(), "http" | "https") {
        return Err(wrong());
    }
    if ["arxiv.org", "www.arxiv.org", "export.arxiv.org"].contains(&host.as_str()) {
        for prefix in ["/abs/", "/pdf/", "/html/"] {
            if let Some(paper) = url.path().strip_prefix(prefix) {
                let paper = paper.trim_end_matches('/').trim_end_matches(".pdf");
                if !paper.is_empty() {
                    return Ok(format!("https://arxiv.org/html/{paper}"));
                }
            }
        }
    }
    Ok(url.to_string())
}

/// Ranks the sections against what the chat is looking for: the likeliest first, each
/// with its probability, and the model that answered. A document with more sections
/// than one question takes is narrowed by its top-level sections first.
pub async fn pick(
    ask: Asker<'_>,
    settings: &Settings,
    paper: &Paper,
    looking_for: &str,
    goal: &str,
) -> Result<(String, Vec<(usize, f64)>), String> {
    let all: Vec<usize> = (0..paper.sections.len()).collect();
    let candidates = if all.len() <= MAX_OPTIONS {
        all
    } else {
        let top = paper
            .sections
            .iter()
            .map(|s| s.path.len())
            .min()
            .unwrap_or(1);
        let chapters: Vec<usize> = all
            .into_iter()
            .filter(|&i| paper.sections[i].path.len() == top)
            .take(MAX_OPTIONS)
            .collect();
        let (_, ranked) = rank(ask, settings, paper, looking_for, goal, &chapters).await?;
        ranked
            .iter()
            .take(2)
            .flat_map(|(chapter, _)| paper.under(*chapter))
            .take(MAX_OPTIONS)
            .collect()
    };
    rank(ask, settings, paper, looking_for, goal, &candidates).await
}

/// The state and pick-one question that rank `candidates`.
pub fn request(
    paper: &Paper,
    looking_for: &str,
    goal: &str,
    candidates: &[usize],
) -> (Value, Value) {
    let options: Map<String, Value> = candidates
        .iter()
        .map(|&i| {
            let start: String = paper.sections[i]
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(PREVIEW)
                .collect();
            let description = if start.is_empty() {
                paper.heading(i)
            } else {
                format!("{}: {start}", paper.heading(i))
            };
            (id(i), json!(description))
        })
        .collect();
    let state = json!({"document": paper.title, "research_goal": goal, "looking_for": looking_for});
    let question = json!({
        "type": "choice",
        "instructions": format!("Someone is reading this document for: {looking_for}. Which section answers that best? Each option is a section: its heading path, then how it starts."),
        "criteria": options,
    });
    (state, question)
}

async fn rank(
    ask: Asker<'_>,
    settings: &Settings,
    paper: &Paper,
    looking_for: &str,
    goal: &str,
    candidates: &[usize],
) -> Result<(String, Vec<(usize, f64)>), String> {
    if let [only] = candidates {
        return Ok((String::new(), vec![(*only, 1.0)]));
    }
    let (state, question) = request(paper, looking_for, goal, candidates);
    let response = ask
        .ask(settings, state, json!({"section": question}), TIMEOUT)
        .await?;
    let picked = choice(&response, "section")?;
    let ranked = picked
        .options
        .iter()
        .filter_map(|(option, p)| {
            let n = option.strip_prefix('s')?.parse::<usize>().ok()?;
            candidates.contains(&(n - 1)).then_some((n - 1, *p))
        })
        .collect();
    Ok((picked.model, ranked))
}

/// The sections to return from a ranking: the likeliest, and up to PICKS in all that
/// are at least FLOOR likely.
pub fn top(ranked: &[(usize, f64)]) -> Vec<(usize, f64)> {
    ranked
        .iter()
        .enumerate()
        .filter(|(k, (_, p))| *k == 0 || *p >= FLOOR)
        .map(|(_, r)| *r)
        .take(PICKS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper() -> Paper {
        let section = |path: &[&str], text: &str| Section {
            level: path.len() as u8 + 1,
            path: path.iter().map(|p| p.to_string()).collect(),
            text: text.into(),
        };
        Paper {
            title: Some("Warmup matters".into()),
            sections: vec![
                section(&["Abstract"], "We study warmup."),
                section(&["1 Introduction"], "Training is hard."),
                section(&["2 Method"], ""),
                section(
                    &["2 Method", "2.1 Schedule"],
                    "Linear warmup for 500 steps.",
                ),
                section(&["2 Method", "2.2 Data"], "Deduplicated."),
                section(&["3 Results"], "Loss 2.31."),
            ],
        }
    }

    #[test]
    fn arxiv_links_become_its_html_pages() {
        for link in [
            "https://arxiv.org/abs/2609.03874",
            "https://arxiv.org/abs/2609.03874v2/",
            "http://www.arxiv.org/pdf/2609.03874.pdf",
            "https://arxiv.org/html/2609.03874",
        ] {
            assert!(
                source(link)
                    .unwrap()
                    .starts_with("https://arxiv.org/html/2609.03874"),
                "{link}"
            );
        }
        assert_eq!(
            source(" https://example.com/post ").unwrap(),
            "https://example.com/post"
        );
        for wrong in [
            "file:///etc/passwd",
            "ftp://example.com/a",
            "arxiv 2609.03874",
            "",
        ] {
            assert!(source(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn sections_are_found_by_id_number_or_heading() {
        let p = paper();
        assert_eq!(p.find("s4"), Some(3));
        assert_eq!(p.find("2.1"), Some(3));
        assert_eq!(p.find("§2.2"), Some(4));
        assert_eq!(p.find("results"), Some(5));
        assert_eq!(p.find("Schedule"), Some(3));
        assert_eq!(p.find("s9"), None);
        assert_eq!(p.find("appendix"), None);
        assert_eq!(p.start(), 0);
    }

    #[test]
    fn a_section_comes_with_what_is_under_it() {
        let p = paper();
        assert_eq!(
            p.body(2),
            "#### 2.1 Schedule\nLinear warmup for 500 steps.\n\n#### 2.2 Data\nDeduplicated."
        );
        assert_eq!(p.body(3), "Linear warmup for 500 steps.");
        assert_eq!(p.under(2).collect::<Vec<_>>(), [2, 3, 4]);
        assert_eq!(
            p.outline(),
            "s1 Abstract\ns2 1 Introduction\ns3 2 Method\ns4   2.1 Schedule\ns5   2.2 Data\ns6 3 Results"
        );
        assert_eq!(p.heading(3), "2 Method › 2.1 Schedule");
    }

    #[test]
    fn the_likeliest_sections_are_kept() {
        assert_eq!(
            top(&[(3, 0.6), (5, 0.3), (1, 0.06), (0, 0.04)]),
            [(3, 0.6), (5, 0.3), (1, 0.06)]
        );
        assert_eq!(top(&[(3, 0.02), (5, 0.01)]), [(3, 0.02)]);
        assert_eq!(top(&[]), []);
    }
}
