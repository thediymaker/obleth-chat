//! The report check: whether what a research reply says about its runs matches what the
//! runs printed. Numbers it ties to a run (`loss 2.31` next to "run 4", `run 4: 2.31`, or
//! a table row with a run column) are compared in code, allowing for rounding to the
//! digits written. Comparisons and causes, which code can't check, go to a decision when
//! that check is on. A reply is checked when its turn ends: what doesn't match is shown
//! under it and told to the model in its next research turn.
//!
//! The parser is deliberately narrow. A number is checked only when it is clearly a value
//! of a run, so a statement it can't read is left alone rather than flagged.

use std::{collections::BTreeMap, time::Duration};

use serde_json::{json, Value};
use sqlx::{PgPool, Row};

use super::Research;
use crate::decisions::{yes_no, Asker};

/// Statements read from one reply, and characters of each shown back.
const MAX_CLAIMS: usize = 60;
const MAX_TEXT: usize = 200;
/// Statements sent to a decision at most, and runs described to it.
const MAX_ASKED: usize = 12;
const MAX_RUNS: usize = 60;
const TIMEOUT: Duration = Duration::from_secs(30);
/// Words that may sit between a value's name and the number: "loss of 2.3", "loss was 2.3".
const CONNECTORS: [&str; 11] = [
    "of",
    "was",
    "is",
    "at",
    "reached",
    "hit",
    "about",
    "around",
    "roughly",
    "approximately",
    "now",
];
/// Words just before a value's name that make it some other value than the one the run
/// printed last: "initial loss 4.5", "the baseline's loss 2.41", "target loss 2.0".
const QUALIFIERS: [&str; 26] = [
    "val",
    "validation",
    "valid",
    "train",
    "training",
    "test",
    "eval",
    "initial",
    "starting",
    "start",
    "peak",
    "min",
    "max",
    "minimum",
    "maximum",
    "mean",
    "average",
    "avg",
    "median",
    "first",
    "early",
    "target",
    "goal",
    "previous",
    "baseline",
    "parent",
];
/// Words just after a number that make it a difference or a point along the way:
/// "12% higher", "loss 2.3 at step 500".
const DELTAS: [&str; 30] = [
    "higher",
    "lower",
    "more",
    "less",
    "better",
    "worse",
    "faster",
    "slower",
    "above",
    "below",
    "over",
    "under",
    "up",
    "down",
    "smaller",
    "larger",
    "fewer",
    "times",
    "step",
    "steps",
    "epoch",
    "epochs",
    "iteration",
    "iterations",
    "checkpoint",
    "gain",
    "drop",
    "increase",
    "decrease",
    "margin",
];
/// Words that make a statement a plan, a target or a guess rather than a result.
const HEDGES: [&str; 24] = [
    "should",
    "would",
    "could",
    "might",
    "may",
    "expect",
    "expected",
    "expecting",
    "target",
    "aim",
    "hope",
    "if",
    "until",
    "before",
    "goal",
    "want",
    "need",
    "needs",
    "predicted",
    "estimate",
    "estimated",
    "likely",
    "probably",
    "maybe",
];
/// Words before "run" that make it a verb ("I'll run 3 seeds"), and after its number
/// that make the number a count ("run 3 more options").
const RUN_VERBS: [&str; 11] = [
    "will", "ll", "to", "can", "could", "should", "would", "must", "might", "i", "we",
];
const COUNTED: [&str; 26] = [
    "more",
    "options",
    "seeds",
    "times",
    "experiments",
    "runs",
    "nodes",
    "variants",
    "children",
    "trials",
    "siblings",
    "ideas",
    "configs",
    "configurations",
    "different",
    "other",
    "additional",
    "extra",
    "candidates",
    "steps",
    "epochs",
    "iterations",
    "minutes",
    "hours",
    "seconds",
    "rounds",
];
/// Words that mark a comparison or a cause, which only a decision can check.
const CAUSES: [&str; 11] = [
    "because", "due", "thanks", "caused", "causes", "helped", "helps", "hurt", "hurts", "explains",
    "why",
];
const COMPARISONS: [&str; 18] = [
    "better",
    "worse",
    "best",
    "worst",
    "faster",
    "slower",
    "improved",
    "improves",
    "beat",
    "beats",
    "than",
    "higher",
    "lower",
    "gain",
    "drop",
    "dropped",
    "regressed",
    "matched",
];

/// A number as written: its value, the size of its last digit, and a trailing %.
#[derive(Clone, Debug, PartialEq)]
struct Number {
    value: f64,
    unit: f64,
    percent: bool,
    written: String,
}

impl Number {
    /// Whether `actual` rounds to this number at the digits written.
    fn matches(&self, actual: f64) -> bool {
        let close = |written: f64, actual: f64| {
            (written - actual).abs() <= self.unit / 2.0 + 1e-9 * actual.abs().max(1.0)
        };
        close(self.value, actual) || (self.percent && close(self.value, actual * 100.0))
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Word(String),
    Num(Number),
    Mark(char),
}

/// A value a statement gives a run: the name it is printed under (None for the
/// research metric), and the run it belongs to when the statement says which.
#[derive(Clone, Debug, PartialEq)]
struct Value_ {
    name: Option<String>,
    number: Number,
    run: Option<i32>,
}

/// One statement that cites runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub text: String,
    pub runs: Vec<i32>,
    values: Vec<Value_>,
    words: Vec<String>,
}

/// What a run printed.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub title: String,
    pub parent: Option<String>,
    pub status: String,
    pub values: BTreeMap<String, f64>,
}

/// Reads a number starting at `i`: digits with optional thousands commas, a decimal part,
/// an exponent and a %. None when `i` starts no number or is inside a word ("fp8", "v2").
fn number_at(chars: &[char], i: usize) -> Option<(Number, usize)> {
    let before = i.checked_sub(1).map(|j| chars[j]);
    if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
        return None;
    }
    let mut j = i;
    let negative = matches!(chars.get(j), Some('-' | '−'))
        && chars.get(j + 1).is_some_and(char::is_ascii_digit)
        && before.is_none_or(|c| c.is_whitespace() || "=:(".contains(c));
    if negative {
        j += 1;
    }
    let start = j;
    let mut digits = String::new();
    while let Some(c) = chars.get(j) {
        if c.is_ascii_digit() {
            digits.push(*c);
            j += 1;
        } else if *c == ','
            && j > start
            && chars
                .get(j + 1..j + 4)
                .is_some_and(|d| d.iter().all(char::is_ascii_digit))
            && !chars.get(j + 4).is_some_and(char::is_ascii_digit)
        {
            j += 1;
        } else {
            break;
        }
    }
    if digits.is_empty() {
        return None;
    }
    let mut decimals = 0i32;
    if chars.get(j) == Some(&'.') && chars.get(j + 1).is_some_and(char::is_ascii_digit) {
        digits.push('.');
        j += 1;
        while let Some(c) = chars.get(j).filter(|c| c.is_ascii_digit()) {
            digits.push(*c);
            decimals += 1;
            j += 1;
        }
    }
    let mut exponent = 0i32;
    if matches!(chars.get(j), Some('e' | 'E')) {
        let mut k = j + 1;
        let sign = match chars.get(k) {
            Some('-') => {
                k += 1;
                -1
            }
            Some('+') => {
                k += 1;
                1
            }
            _ => 1,
        };
        let from = k;
        while chars.get(k).is_some_and(char::is_ascii_digit) {
            k += 1;
        }
        if k > from {
            exponent = sign
                * chars[from..k]
                    .iter()
                    .collect::<String>()
                    .parse::<i32>()
                    .ok()?;
            j = k;
        }
    }
    if chars.get(j).is_some_and(|c| c.is_alphabetic() || *c == '_') {
        return None;
    }
    let percent = chars.get(j) == Some(&'%');
    if percent {
        j += 1;
    }
    let mut value = digits.parse::<f64>().ok()? * 10f64.powi(exponent);
    if negative {
        value = -value;
    }
    let written: String = chars[i..j].iter().collect();
    Some((
        Number {
            value,
            unit: 10f64.powi(exponent - decimals),
            percent,
            written,
        },
        j,
    ))
}

fn tokens(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some((number, end)) = number_at(&chars, i) {
            out.push(Token::Num(number));
            i = end;
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while chars
                .get(i)
                .is_some_and(|c| c.is_alphanumeric() || *c == '_' || *c == '.' || *c == '/')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            out.push(Token::Word(
                word.trim_end_matches(['.', '/']).to_lowercase(),
            ));
        } else {
            if !c.is_whitespace() {
                out.push(Token::Mark(c));
            }
            i += 1;
        }
    }
    out
}

fn whole(token: Option<&Token>) -> Option<i32> {
    match token {
        Some(Token::Num(n)) if n.unit == 1.0 && !n.percent && n.value >= 1.0 => {
            Some(n.value as i32)
        }
        _ => None,
    }
}

/// The runs a statement cites ("run 4", "runs 3, 5 and 7"), and the values it ties to them.
fn read(text: &str, keys: &[String]) -> Option<Claim> {
    let t = tokens(text);
    let mut runs = vec![];
    let mut values = vec![];
    let mut i = 0;
    while i < t.len() {
        match &t[i] {
            Token::Word(w) if w == "run" || w == "runs" => {
                let verb =
                    i > 0 && matches!(&t[i - 1], Token::Word(p) if RUN_VERBS.contains(&p.as_str()));
                let mut j = i + 1;
                let mut found = vec![];
                loop {
                    if matches!(t.get(j), Some(Token::Mark('#'))) {
                        j += 1;
                    }
                    let Some(n) = whole(t.get(j)) else { break };
                    found.push(n);
                    j += 1;
                    // A range: runs 3-5.
                    if let (Some(Token::Mark('-' | '–' | '—')), Some(m)) =
                        (t.get(j), whole(t.get(j + 1)))
                    {
                        if m > n && m - n <= 30 {
                            found.extend(n + 1..=m);
                            j += 2;
                        }
                    }
                    if w == "run" {
                        break;
                    }
                    match t.get(j) {
                        Some(Token::Mark(',' | '&' | '/')) => j += 1,
                        Some(Token::Word(a)) if a == "and" => j += 1,
                        _ => break,
                    }
                    if matches!(t.get(j), Some(Token::Word(a)) if a == "and") {
                        j += 1;
                    }
                }
                let count =
                    matches!(t.get(j), Some(Token::Word(next)) if COUNTED.contains(&next.as_str()));
                if !verb && !count {
                    for n in &found {
                        if !runs.contains(n) {
                            runs.push(*n);
                        }
                    }
                    // "run 4: 2.31" gives the research metric of run 4.
                    if let (Some(&run), Some(Token::Mark(':'))) = (found.last(), t.get(j)) {
                        if let Some(Token::Num(n)) = t.get(j + 1) {
                            values.push(Value_ {
                                name: None,
                                number: n.clone(),
                                run: Some(run),
                            });
                            j += 2;
                        }
                    }
                }
                i = j.max(i + 1);
            }
            Token::Word(w) if keys.contains(w) => {
                let mut j = i + 1;
                let mut skipped = 0;
                while skipped < 3 {
                    match t.get(j) {
                        Some(Token::Mark(m)) if "=:(*`~≈".contains(*m) => {}
                        Some(Token::Word(a)) if CONNECTORS.contains(&a.as_str()) => {}
                        _ => break,
                    }
                    j += 1;
                    skipped += 1;
                }
                if let Some(Token::Num(n)) = t.get(j) {
                    let word_in = |range: &[Token], list: &[&str]| {
                        range
                            .iter()
                            .any(|t| matches!(t, Token::Word(q) if list.contains(&q.as_str())))
                    };
                    // The word just before the name, or before its 's.
                    let before = match &t[..i] {
                        [.., Token::Word(q), Token::Mark('\'' | '’'), Token::Word(s)]
                            if s == "s" =>
                        {
                            Some(q)
                        }
                        [.., Token::Word(q)] => Some(q),
                        _ => None,
                    };
                    let qualified = before.is_some_and(|q| QUALIFIERS.contains(&q.as_str()));
                    let after = t[j + 1..]
                        .iter()
                        .take(2)
                        .take_while(|t| matches!(t, Token::Word(_)))
                        .count();
                    let delta = word_in(&t[j + 1..j + 1 + after], &DELTAS)
                        || matches!(t.get(j - 1), Some(Token::Mark('+' | '±')));
                    if !qualified && !delta {
                        values.push(Value_ {
                            name: Some(w.clone()),
                            number: n.clone(),
                            run: None,
                        });
                    }
                    i = j + 1;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    if runs.is_empty() {
        return None;
    }
    let words = t
        .into_iter()
        .filter_map(|t| match t {
            Token::Word(w) => Some(w),
            _ => None,
        })
        .collect();
    Some(Claim {
        text: shorten(text),
        runs,
        values,
        words,
    })
}

fn shorten(text: &str) -> String {
    let text = text.trim().trim_start_matches(['-', '*', '•', ' ']).trim();
    if text.chars().count() <= MAX_TEXT {
        return text.to_owned();
    }
    let cut: String = text.chars().take(MAX_TEXT - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Splits a line into sentences at ., ! or ? followed by a capital letter.
fn sentences(line: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let bytes: Vec<(usize, char)> = line.char_indices().collect();
    for (k, (at, c)) in bytes.iter().enumerate() {
        if !".!?".contains(*c) {
            continue;
        }
        let rest = &bytes[k + 1..];
        let spaces = rest.iter().take_while(|(_, c)| c.is_whitespace()).count();
        if spaces > 0
            && rest
                .get(spaces)
                .is_some_and(|(_, c)| c.is_uppercase() || "(*[".contains(*c))
        {
            let end = at + c.len_utf8();
            out.push(&line[start..end]);
            start = rest[spaces].0;
        }
    }
    out.push(&line[start..]);
    out
}

fn cells(row: &str) -> Vec<String> {
    let row = row.trim().trim_start_matches('|').trim_end_matches('|');
    row.split('|')
        .map(|c| c.trim().trim_matches(['*', '`', '_']).trim().to_owned())
        .collect()
}

/// A table row with a run column: each column named after a printed value gives that
/// value for the row's run.
fn row(header: &[String], line: &str, keys: &[String]) -> Option<Claim> {
    let lower: Vec<String> = header.iter().map(|h| h.to_lowercase()).collect();
    let run_column = lower.iter().position(|h| {
        let h = h.trim_matches(['#', ' ']);
        h == "run" || h == "runs" || h.is_empty() && lower[0] == "#" || h.starts_with("run ")
    })?;
    let row = cells(line);
    let run = tokens(row.get(run_column)?)
        .iter()
        .find_map(|t| whole(Some(t)))?;
    let mut values = vec![];
    for (k, head) in lower.iter().enumerate() {
        let names: Vec<String> = tokens(head)
            .into_iter()
            .filter_map(|t| match t {
                Token::Word(w) => Some(w),
                _ => None,
            })
            .collect();
        let Some(key) = keys.iter().find(|key| names.first() == Some(key)) else {
            continue;
        };
        if let Some(Token::Num(n)) = row.get(k).and_then(|cell| {
            tokens(cell)
                .into_iter()
                .find(|t| matches!(t, Token::Num(_)))
        }) {
            values.push(Value_ {
                name: Some(key.clone()),
                number: n,
                run: Some(run),
            });
        }
    }
    let mut claim = read(&row.join(" · "), keys).unwrap_or(Claim {
        text: String::new(),
        runs: vec![],
        values: vec![],
        words: vec![],
    });
    claim.text = shorten(&row.join(" · "));
    if !claim.runs.contains(&run) {
        claim.runs.insert(0, run);
    }
    claim.values.extend(values);
    Some(claim)
}

/// Every statement in a reply that cites runs, with code blocks left out.
pub fn claims(reply: &str, keys: &[String]) -> Vec<Claim> {
    let mut out = vec![];
    let mut fenced = false;
    let lines: Vec<&str> = reply.lines().collect();
    let mut i = 0;
    while i < lines.len() && out.len() < MAX_CLAIMS {
        let line = lines[i].trim();
        if line.starts_with("```") {
            fenced = !fenced;
            i += 1;
            continue;
        }
        if fenced {
            i += 1;
            continue;
        }
        let separator = |l: &str| {
            l.starts_with('|') && l.chars().all(|c| "|-: ".contains(c)) && l.contains('-')
        };
        if line.starts_with('|') && lines.get(i + 1).is_some_and(|l| separator(l.trim())) {
            let header = cells(line);
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                if let Some(claim) = row(&header, lines[i], keys) {
                    out.push(claim);
                } else {
                    out.extend(read(&cells(lines[i]).join(" "), keys));
                }
                i += 1;
            }
            continue;
        }
        for sentence in sentences(line) {
            out.extend(read(sentence, keys));
        }
        i += 1;
    }
    out.truncate(MAX_CLAIMS);
    out
}

fn shown(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.into()
    }
}

/// What code can say about a statement: its problems, and whether a value of it was checked.
pub fn verify(claim: &Claim, runs: &BTreeMap<i32, Facts>, metric: &str) -> (Vec<String>, bool) {
    let mut problems = vec![];
    for r in &claim.runs {
        if !runs.contains_key(r) {
            problems.push(format!("There is no run {r}."));
        }
    }
    // A plan, a target or a guess isn't a result, so its numbers aren't checked.
    let hedged = claim.words.iter().any(|w| HEDGES.contains(&w.as_str()));
    let mut checked = false;
    for value in claim.values.iter().filter(|_| !hedged) {
        let key = value.name.as_deref().unwrap_or(metric);
        let candidates: Vec<i32> = match value.run {
            Some(r) => vec![r],
            None => claim.runs.clone(),
        };
        // A running run's values change as it goes, so they aren't checked.
        let known: Vec<i32> = candidates
            .into_iter()
            .filter(|r| runs.get(r).is_some_and(|f| f.status != "running"))
            .collect();
        if known.is_empty() {
            continue;
        }
        let printed: Vec<(i32, f64)> = known
            .iter()
            .filter_map(|r| Some((*r, *runs[r].values.get(key)?)))
            .collect();
        if printed.is_empty() {
            problems.push(match known.as_slice() {
                [r] => match runs[r].status.as_str() {
                    "failed" => format!("Run {r} printed no {key}: it failed."),
                    "cancelled" | "lost" => {
                        format!("Run {r} printed no {key}: it was {}.", runs[r].status)
                    }
                    _ => format!("Run {r} printed no {key}."),
                },
                more => format!(
                    "None of runs {} printed {key}.",
                    more.iter()
                        .map(|r| r.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
            continue;
        }
        checked = true;
        // With several runs in the statement, a value one of them didn't print may be its.
        if printed.iter().any(|(_, a)| value.number.matches(*a)) || printed.len() < known.len() {
            continue;
        }
        let mut what = printed
            .iter()
            .map(|(r, a)| format!("run {r} printed {key}={}", shown(*a)))
            .collect::<Vec<_>>()
            .join(", ");
        what[..1].make_ascii_uppercase();
        problems.push(format!("{what}, not {}.", value.number.written));
    }
    (problems, checked)
}

/// Whether a statement makes a comparison or gives a cause only a decision can check.
fn judgment(claim: &Claim, checked: bool) -> bool {
    let has = |list: &[&str]| claim.words.iter().any(|w| list.contains(&w.as_str()));
    has(&CAUSES) || (!checked && has(&COMPARISONS))
}

/// Checks a research reply when it cites runs. Records the problems for the next turn and
/// returns the step to show under the reply; None when the reply cites no run.
pub async fn check(
    pool: &PgPool,
    ask: Option<Asker<'_>>,
    research: &Research,
    reply: &str,
) -> Option<Value> {
    if !reply.to_lowercase().contains("run") {
        save(pool, research, None).await;
        return None;
    }
    let (began, started) = (std::time::Instant::now(), chrono::Utc::now());
    let rows = sqlx::query("SELECT r.number, r.status, r.metric, r.metrics, e.title, p.title AS parent FROM research_runs r JOIN experiments e ON e.id=r.experiment_id LEFT JOIN experiments p ON p.id=e.parent_id WHERE r.research_id=$1 ORDER BY r.number")
        .bind(research.id)
        .fetch_all(pool)
        .await
        .ok()?;
    let metric = research.metric.to_lowercase();
    let mut runs = BTreeMap::new();
    for r in &rows {
        let mut values: BTreeMap<String, f64> = r
            .get::<Value, _>("metrics")
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some((k.to_lowercase(), v.as_f64()?)))
            .collect();
        if let Some(m) = r.get::<Option<f64>, _>("metric") {
            values.insert(metric.clone(), m);
        }
        runs.insert(
            r.get::<i32, _>("number"),
            Facts {
                title: r.get("title"),
                parent: r.get("parent"),
                status: r.get("status"),
                values,
            },
        );
    }
    let mut keys: Vec<String> = runs
        .values()
        .flat_map(|f| f.values.keys().cloned())
        .chain([metric.clone()])
        .collect();
    keys.sort();
    keys.dedup();
    let claims = claims(reply, &keys);
    if claims.is_empty() {
        save(pool, research, None).await;
        return None;
    }
    let mut problems: Vec<(String, String)> = vec![];
    let mut asked = vec![];
    for claim in &claims {
        let (found, checked) = verify(claim, &runs, &metric);
        if found.is_empty() {
            if judgment(claim, checked) && asked.len() < MAX_ASKED {
                asked.push(claim);
            }
        } else {
            problems.push((claim.text.clone(), found.join(" ")));
        }
    }
    let mut note = String::new();
    if let Some(ask) = ask.filter(|_| !asked.is_empty()) {
        match ask.settings().await {
            Some(settings) if settings.on() && settings.check_reports => {
                match judge(ask, &settings, research, &runs, &asked).await {
                    Ok(doubted) => problems.extend(doubted),
                    Err(why) => {
                        note = format!(
                            "\n{} comparison{} and cause{} not checked: {why}",
                            asked.len(),
                            if asked.len() == 1 { "" } else { "s" },
                            if asked.len() == 1 { "" } else { "s" },
                        )
                    }
                }
            }
            _ => {}
        }
    }
    let n = claims.len();
    let statements = if n == 1 { "statement" } else { "statements" };
    let summary = match problems.len() {
        0 => format!("Checked {n} {statements} against the runs: they match"),
        1 => format!("Checked {n} {statements} against the runs: 1 doesn't match"),
        k => format!("Checked {n} {statements} against the runs: {k} don't match"),
    };
    let detail = problems
        .iter()
        .map(|(text, problem)| format!("“{text}”\n  {problem}"))
        .collect::<Vec<_>>()
        .join("\n");
    save(
        pool,
        research,
        (!problems.is_empty()).then(|| {
            json!({"checked": n, "problems": problems.iter().map(|(t, p)| json!({"text": t, "problem": p})).collect::<Vec<_>>()})
        }),
    )
    .await;
    Some(json!({
        "id": "report-check",
        "tool": "report_check",
        "summary": summary,
        "status": "completed",
        "flagged": !problems.is_empty(),
        "detail": format!("{detail}{note}").trim().to_owned(),
        "startedAt": started,
        "durationMs": began.elapsed().as_millis() as u64,
    }))
}

/// Asks whether the runs support each statement; returns the ones the answer doubts.
async fn judge(
    ask: Asker<'_>,
    settings: &crate::decisions::Settings,
    research: &Research,
    runs: &BTreeMap<i32, Facts>,
    asked: &[&Claim],
) -> Result<Vec<(String, String)>, String> {
    let table: Vec<Value> = runs
        .iter()
        .rev()
        .take(MAX_RUNS)
        .rev()
        .map(|(n, f)| {
            json!({"run": n, "idea": f.title, "builds_on": f.parent, "status": f.status,
                "printed": f.values.iter().take(12).map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>()})
        })
        .collect();
    let state = json!({
        "goal": research.goal,
        "metric": format!("{} ({} is better)", research.metric, if research.direction == "min" { "lower" } else { "higher" }),
        "runs": table,
    });
    let questions: serde_json::Map<String, Value> = asked
        .iter()
        .enumerate()
        .map(|(k, claim)| {
            (format!("s{}", k + 1), json!({
                "type": "boolean",
                "instructions": format!("This statement is from the research's report: “{}”. Do the runs support it?", claim.text),
                "criteria": {
                    "true": "Yes: the results it states match the runs, and any comparison or cause it gives follows from the runs or is labelled as a guess.",
                    "false": "No: it states a result, comparison or cause that the runs contradict or don't show, and doesn't label it as a guess."
                }
            }))
        })
        .collect();
    let response = ask
        .ask(settings, state, Value::Object(questions), TIMEOUT)
        .await?;
    let mut doubted = vec![];
    for (k, claim) in asked.iter().enumerate() {
        if let Ok(v) = yes_no(&response, &format!("s{}", k + 1)) {
            if v.sure_no() {
                let who = if v.model.is_empty() {
                    "A check".to_string()
                } else {
                    format!("A check by {}", v.model)
                };
                doubted.push((
                    claim.text.clone(),
                    format!(
                        "{who} doubts the runs support this: it gives {:.0}% that they do.",
                        v.yes * 100.0
                    ),
                ));
            }
        }
    }
    Ok(doubted)
}

async fn save(pool: &PgPool, research: &Research, found: Option<Value>) {
    let _ = sqlx::query("UPDATE research SET report_check=$2 WHERE id=$1")
        .bind(research.id)
        .bind(found)
        .execute(pool)
        .await;
}

/// The prompt section that tells the model what its last report got wrong.
pub fn prompt(found: &Value) -> Option<String> {
    let problems = found["problems"].as_array().filter(|p| !p.is_empty())?;
    let list = problems
        .iter()
        .map(|p| {
            format!(
                "- “{}”: {}",
                p["text"].as_str().unwrap_or(""),
                p["problem"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!("## Your last report\nA check found statements in your last reply that the runs don't support:\n{list}\nCorrect them in this reply, with the values from research_status or run_log, and label guesses as guesses."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(status: &str, values: &[(&str, f64)]) -> Facts {
        Facts {
            title: "idea".into(),
            parent: None,
            status: status.into(),
            values: values.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    fn table() -> BTreeMap<i32, Facts> {
        BTreeMap::from([
            (1, facts("completed", &[("loss", 2.4113), ("lr", 0.0003)])),
            (
                2,
                facts(
                    "completed",
                    &[("loss", 2.3148), ("lr", 0.001), ("tok_s", 1534.0)],
                ),
            ),
            (3, facts("failed", &[])),
        ])
    }

    fn keys() -> Vec<String> {
        ["loss", "lr", "tok_s"].map(String::from).to_vec()
    }

    fn problems(text: &str) -> Vec<String> {
        claims(text, &keys())
            .iter()
            .flat_map(|c| verify(c, &table(), "loss").0)
            .collect()
    }

    #[test]
    fn numbers_keep_the_digits_they_were_written_with() {
        let n = |s: &str| number_at(&s.chars().collect::<Vec<_>>(), 0).unwrap().0;
        assert_eq!((n("2.31").value, n("2.31").unit), (2.31, 0.01));
        assert_eq!(n("1e-3").value, 0.001);
        assert_eq!(n("1,534").value, 1534.0);
        assert!(n("93.1%").percent);
        assert!(n("2.31").matches(2.3148));
        assert!(!n("2.32").matches(2.3148));
        assert!(n("2.3").matches(2.3148));
        assert!(n("93.1%").matches(0.931));
        // Inside a word it isn't a number.
        let chars: Vec<char> = "fp8".chars().collect();
        assert!(number_at(&chars, 2).is_none());
    }

    #[test]
    fn statements_cite_runs_in_prose_lists_and_the_status_format() {
        let found = claims(
            "Run 2 reached loss 2.31, down from 2.41 in run 1. Runs 1, 2 and 3 used one command.\n- run 2: 2.3148 · the best so far\n```\nrun 9 loss=1.0\n```",
            &keys(),
        );
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].runs, vec![2, 1]);
        assert_eq!(found[1].runs, vec![1, 2, 3]);
        assert_eq!(found[2].values[0].run, Some(2));
        assert!(problems("Run 2 reached loss 2.31, down from 2.41 in run 1.").is_empty());
        assert!(verify(&found[0], &table(), "loss").1);
    }

    #[test]
    fn wrong_values_missing_runs_and_failed_runs_are_named() {
        assert_eq!(
            problems("Run 2 got loss=2.29 with lr 1e-3."),
            vec!["Run 2 printed loss=2.3148, not 2.29."]
        );
        assert_eq!(problems("Run 12 is the best."), vec!["There is no run 12."]);
        assert_eq!(
            problems("Run 1 is the baseline at loss 2.45."),
            vec!["Run 1 printed loss=2.4113, not 2.45."]
        );
        assert_eq!(
            problems("Run 3 reached loss 2.2."),
            vec!["Run 3 printed no loss: it failed."]
        );
        assert_eq!(
            problems("run 1: 2.39"),
            vec!["Run 1 printed loss=2.4113, not 2.39."]
        );
        // A value the run didn't print, under a name no run printed, isn't checked.
        assert!(problems("Run 2 used batch 64.").is_empty());
    }

    #[test]
    fn plans_references_and_differences_are_left_alone() {
        for text in [
            "I'll run 3 more seeds next.",
            "Next, run 2 options of the warmup.",
            "Run 2 should reach loss 2.2 once the warmup lands.",
            "Run 2 reached loss 2.31, below the baseline loss 2.41.",
            "Run 2 reached loss 2.31, below the baseline's loss 2.41.",
            "Run 2 started from an initial loss 4.5.",
            "Run 2 is 4% higher in tok_s.",
            "Run 2 cut loss from 2.41 to 2.31.",
            "Run 2 had loss 2.9 at step 100.",
            "Run 3 crashed, and run 2 printed loss 2.31.",
        ] {
            assert!(problems(text).is_empty(), "{text}: {:?}", problems(text));
        }
        let mut runs = table();
        runs.get_mut(&2).unwrap().status = "running".into();
        let claim = claims("Run 2 is at loss 2.9 so far.", &keys()).remove(0);
        assert!(verify(&claim, &runs, "loss").0.is_empty());
    }

    #[test]
    fn runs_are_cited_by_number_hash_and_range() {
        let found = claims("Runs #1-3 used one command, run #2 too.", &keys());
        assert_eq!(found[0].runs, vec![1, 2, 3]);
        assert_eq!(problems("Runs 2-4 finished."), vec!["There is no run 4."]);
    }

    #[test]
    fn tables_with_a_run_column_are_checked_row_by_row() {
        let text = "| Run | Idea | loss | tok/s |\n|---|---|---|---|\n| 1 | baseline | 2.41 | — |\n| 2 | lr 1e-3 | **2.30** | 1,534 |";
        let found = claims(text, &keys());
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].runs, vec![2]);
        assert_eq!(
            found
                .iter()
                .flat_map(|c| verify(c, &table(), "loss").0)
                .collect::<Vec<_>>(),
            vec!["Run 2 printed loss=2.3148, not 2.30."]
        );
    }

    #[test]
    fn comparisons_and_causes_go_to_a_decision() {
        let c = |t: &str| claims(t, &keys()).remove(0);
        assert!(judgment(
            &c("Run 2 beat run 1 because the warmup helped."),
            true
        ));
        assert!(judgment(&c("Run 2 was faster than run 1."), false));
        // A comparison its numbers already show needs no decision.
        assert!(!judgment(&c("Run 2 is better: loss 2.31."), true));
        assert!(!judgment(&c("Run 2 finished."), false));
    }

    #[test]
    fn the_prompt_lists_what_to_correct() {
        let text = prompt(&json!({"checked": 3, "problems": [{"text": "Run 2 got loss=2.29", "problem": "Run 2 printed loss=2.3148, not 2.29."}]})).unwrap();
        assert!(text.contains("- “Run 2 got loss=2.29”: Run 2 printed loss=2.3148, not 2.29."));
        assert!(prompt(&json!({"checked": 3, "problems": []})).is_none());
    }
}
