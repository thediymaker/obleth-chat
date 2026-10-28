//! What a model is told about a file it is not sent: its kind, an archive's list of
//! entries, or the start of a text file. Worked out once, on upload, from the bytes
//! alone: nothing is unpacked or inflated.

use super::size_label;

/// Entries of an archive listed, and the most text the list may take.
const MAX_ENTRIES: usize = 200;
const MAX_LISTING_BYTES: usize = 8 * 1024;
/// How much of a text file the model sees.
const PREVIEW_BYTES: usize = 4 * 1024;

/// The note kept with a file (attachments.extracted_text) and, for an archive, how
/// many entries it holds (attachments.parts).
pub(super) fn describe(name: &str, data: &[u8]) -> (String, Option<i32>) {
    let kind = kind(name, data);
    if data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06") {
        if let Some((count, entries)) = crate::imports::zip::listing(data, MAX_ENTRIES) {
            return (listed(kind, count, &entries), i32::try_from(count).ok());
        }
    }
    if is_tar(data) {
        let (count, entries) = tar_listing(data);
        return (listed(kind, count, &entries), i32::try_from(count).ok());
    }
    match preview(data) {
        Some((text, true)) => (
            format!("{}. All of it:\n{text}\n[End of the file.]", sentence(kind)),
            None,
        ),
        Some((text, false)) => (
            format!(
                "{}. Its first {}:\n{text}\n[The file continues.]",
                sentence(kind),
                size_label(text.len())
            ),
            None,
        ),
        None => (format!("{}.", sentence(kind)), None),
    }
}

fn sentence(kind: &str) -> String {
    let article = if kind.starts_with(['a', 'e', 'i', 'o', 'u', 'A', 'E', 'I', 'O', 'U']) {
        "An"
    } else {
        "A"
    };
    format!("{article} {kind}")
}

fn listed(kind: &str, count: u64, entries: &[(String, u64)]) -> String {
    let mut text = format!(
        "{} with {count} entr{}. Entries (path and size):\n",
        sentence(kind),
        if count == 1 { "y" } else { "ies" }
    );
    let mut shown = 0u64;
    for (path, size) in entries {
        let line = format!("{} ({})\n", clean(path), size_label(*size as usize));
        if text.len() + line.len() > MAX_LISTING_BYTES {
            break;
        }
        text.push_str(&line);
        shown += 1;
    }
    if count > shown {
        text.push_str(&format!("… and {} more.\n", count - shown));
    }
    text.trim_end().to_owned()
}

/// An entry's path on one line.
fn clean(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .take(300)
        .collect()
}

/// What the file is, from its name and first bytes.
fn kind(name: &str, data: &[u8]) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map_or("", |(_, ext)| ext);
    if lower.ends_with(".tar.gz") || extension == "tgz" {
        return "gzip-compressed tar archive";
    }
    if lower.ends_with(".tar.bz2") || lower.ends_with(".tar.xz") || lower.ends_with(".tar.zst") {
        return "compressed tar archive";
    }
    match extension {
        "zip" => "zip archive",
        "tar" => "tar archive",
        "gz" => "gzip-compressed file",
        "bz2" | "xz" | "zst" => "compressed file",
        "7z" => "7-Zip archive",
        "rar" => "RAR archive",
        "jar" | "war" => "Java archive",
        "whl" => "Python wheel",
        "csv" => "CSV file",
        "tsv" => "tab-separated file",
        "json" => "JSON file",
        "jsonl" | "ndjson" => "JSON Lines file",
        "parquet" => "Parquet data file",
        "h5" | "hdf5" | "he5" => "HDF5 data file",
        "nc" => "NetCDF data file",
        "npy" => "NumPy array file",
        "npz" => "NumPy archive",
        "pkl" | "pickle" => "Python pickle (do not load one you do not trust)",
        "feather" | "arrow" => "Arrow data file",
        "sqlite" | "db" => "SQLite database",
        "log" => "log file",
        "svg" => "SVG image",
        "gif" | "bmp" | "tif" | "tiff" | "heic" | "avif" | "ico" => "image",
        "png" | "jpg" | "jpeg" | "webp" => "image too large, or not complete enough, to view",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => "audio file",
        "mp4" | "mov" | "webm" | "mkv" | "avi" => "video file",
        "doc" | "ppt" | "xls" => "older Office file",
        "odt" | "ods" | "odp" => "OpenDocument file",
        "ipynb" => "Jupyter notebook",
        "html" | "htm" => "HTML page",
        _ if data.starts_with(b"\x7fELF") => "Linux program",
        _ if data.starts_with(b"\x1f\x8b") => "gzip-compressed file",
        _ if preview(data).is_some() => "text file",
        _ => "file",
    }
}

fn is_tar(data: &[u8]) -> bool {
    data.len() >= 512 && data.get(257..262) == Some(b"ustar")
}

/// Entries of an uncompressed tar file, walking its headers and skipping their data.
fn tar_listing(data: &[u8]) -> (u64, Vec<(String, u64)>) {
    let mut entries = vec![];
    let (mut count, mut at) = (0u64, 0usize);
    let mut long_name: Option<String> = None;
    while let Some(header) = data.get(at..at + 512) {
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let Some(size) = octal(&header[124..136]) else {
            break;
        };
        let body = at + 512;
        let next = body.saturating_add(size.div_ceil(512).saturating_mul(512) as usize);
        match header[156] {
            // A GNU long name, for the entry that follows.
            b'L' => {
                long_name = data.get(body..body + (size as usize).min(4096)).map(field);
            }
            // Extended headers say nothing worth listing.
            b'x' | b'g' => {}
            kind => {
                let mut name = long_name.take().unwrap_or_else(|| {
                    let prefix = field(&header[345..500]);
                    let name = field(&header[..100]);
                    if prefix.is_empty() {
                        name
                    } else {
                        format!("{prefix}/{name}")
                    }
                });
                if kind == b'5' && !name.ends_with('/') {
                    name.push('/');
                }
                count += 1;
                if entries.len() < MAX_ENTRIES {
                    entries.push((name, size));
                }
            }
        }
        at = next;
    }
    (count, entries)
}

fn field(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// A tar size: octal digits, or a base-256 number when the first bit is set.
fn octal(bytes: &[u8]) -> Option<u64> {
    if bytes[0] & 0x80 != 0 {
        return Some(bytes[1..].iter().fold(0u64, |n, &b| {
            n.saturating_mul(256).saturating_add(u64::from(b))
        }));
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let digits = text.trim_matches(|c: char| c == '\0' || c == ' ');
    if digits.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(digits, 8).ok()
}

/// The start of a UTF-8 text file, and whether that is all of it. None for binary data.
fn preview(data: &[u8]) -> Option<(&str, bool)> {
    let head = &data[..data.len().min(PREVIEW_BYTES)];
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        // A character cut off at the end of the preview is fine.
        Err(error) if error.error_len().is_none() && head.len() < data.len() => {
            std::str::from_utf8(&head[..error.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    if text.is_empty()
        || text
            .chars()
            .any(|c| c.is_control() && !['\n', '\r', '\t', '\u{c}'].contains(&c))
    {
        return None;
    }
    Some((text, head.len() == data.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip with stored entries, enough for its central directory to be read.
    fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut directory) = (vec![], vec![]);
        for (name, data) in entries {
            let offset = out.len() as u32;
            let crc = crc32fast::hash(data);
            let common = |out: &mut Vec<u8>| {
                out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                out.extend_from_slice(&crc.to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend_from_slice(&(name.len() as u16).to_le_bytes());
                out.extend_from_slice(&[0, 0]);
            };
            out.extend_from_slice(b"PK\x03\x04");
            common(&mut out);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            directory.extend_from_slice(b"PK\x01\x02\x14\x00");
            common(&mut directory);
            directory.extend_from_slice(&[0; 10]);
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn a_zip_lists_its_entries_and_counts_them() {
        let data = zip(&[
            ("site/index.html", b"<h1>hi</h1>"),
            ("site/logo.png", &[0; 2048]),
        ]);
        let (text, parts) = describe("site.zip", &data);
        assert_eq!(parts, Some(2));
        assert!(text.starts_with("A zip archive with 2 entries."), "{text}");
        assert!(
            text.contains("site/index.html (1 KB)\nsite/logo.png (2 KB)"),
            "{text}"
        );
    }

    #[test]
    fn a_long_listing_is_cut_with_a_count_of_the_rest() {
        let names: Vec<String> = (0..300).map(|i| format!("data/part-{i:04}.csv")).collect();
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
        let (text, parts) = describe("parts.zip", &zip(&entries));
        assert_eq!(parts, Some(300));
        assert!(text.len() <= MAX_LISTING_BYTES + 200);
        assert!(text.contains("data/part-0000.csv"));
        assert!(!text.contains("data/part-0250.csv"));
        assert!(text.ends_with("… and 100 more."), "{text}");
    }

    #[test]
    fn a_tar_lists_its_entries() {
        let mut data = vec![];
        for (name, size, kind) in [("code/", 0usize, b'5'), ("code/main.rs", 700, b'0')] {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            header[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
            header[156] = kind;
            header[257..262].copy_from_slice(b"ustar");
            data.extend_from_slice(&header);
            data.extend(std::iter::repeat_n(b'x', size.div_ceil(512) * 512));
        }
        data.extend_from_slice(&[0; 1024]);
        let (text, parts) = describe("code.tar", &data);
        assert_eq!(parts, Some(2));
        assert!(text.contains("A tar archive with 2 entries."), "{text}");
        assert!(text.contains("code/ (0 KB)\ncode/main.rs (1 KB)"), "{text}");
    }

    #[test]
    fn text_shows_its_start_and_binary_only_its_kind() {
        let csv = format!("id,value\n{}", "1,2.5\n".repeat(2000));
        let (text, parts) = describe("results.csv", csv.as_bytes());
        assert_eq!(parts, None);
        assert!(text.starts_with("A CSV file. Its first 4 KB:\nid,value\n1,2.5\n"));
        assert!(text.ends_with("[The file continues.]"));
        let (text, _) = describe("notes.txt", "é".repeat(3000).as_bytes());
        assert!(text.ends_with("[The file continues.]"));
        let (text, _) = describe("small.log", b"started\n");
        assert_eq!(
            text,
            "A log file. All of it:\nstarted\n\n[End of the file.]"
        );
        assert_eq!(
            describe("weights.npy", b"\x93NUMPY\x01\x00").0,
            "A NumPy array file."
        );
        assert_eq!(describe("blob", &[0, 1, 2, 3]).0, "A file.");
        assert_eq!(describe("app", b"\x7fELF\x02").0, "A Linux program.");
    }
}
