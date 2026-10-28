//! Just enough of the zip format to read conversations.json from a ChatGPT or
//! Claude.ai export: the central directory (with zip64 sizes), stored or deflated
//! entries, and the entry's checksum.
use std::io::Read;

use crate::error::ApiError;

/// conversations.json may inflate well past the upload; parsing streams it.
const MAX_ENTRY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub(super) const BROKEN: &str =
    "This zip file could not be read. Unzip it and choose the conversations.json file inside.";

/// The uncompressed conversations.json in a zip, checked as it is read.
pub(super) fn conversations(zip: &[u8]) -> Result<impl Read + '_, ApiError> {
    let broken = || ApiError::bad(BROKEN);
    let entry = find(zip).ok_or_else(broken)?.ok_or_else(|| {
        ApiError::bad("This zip has no conversations.json. Choose the export zip from ChatGPT or Claude, or the conversations.json file inside it.")
    })?;
    if entry.size > MAX_ENTRY_BYTES {
        return Err(ApiError::bad(
            "The conversations in this zip are too large to import.",
        ));
    }
    let local = entry.offset;
    if u32_at(zip, local)? != 0x0403_4b50 {
        return Err(broken());
    }
    let start =
        local + 30 + u64::from(u16_at(zip, local + 26)?) + u64::from(u16_at(zip, local + 28)?);
    let data = usize::try_from(start)
        .ok()
        .zip(usize::try_from(start + entry.compressed).ok())
        .and_then(|(a, b)| zip.get(a..b))
        .ok_or_else(broken)?;
    let reader: Box<dyn Read> = match entry.method {
        0 => Box::new(data),
        8 => Box::new(flate2::read::DeflateDecoder::new(data)),
        _ => {
            return Err(ApiError::bad(
                "This zip uses a kind of compression Chat can't read. Unzip it and choose the conversations.json file inside.",
            ))
        }
    };
    Ok(Checked {
        inner: reader.take(entry.size),
        crc: crc32fast::Hasher::new(),
        expected: entry.crc,
        size: entry.size,
        read: 0,
        done: false,
    })
}

struct Entry {
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    offset: u64,
}

/// The shortest-named conversations.json, outside any macOS resource folder.
fn find(zip: &[u8]) -> Option<Option<Entry>> {
    let mut best: Option<(usize, Entry)> = None;
    walk(zip, |name, entry| {
        let name = String::from_utf8_lossy(name);
        let wanted = (name == "conversations.json" || name.ends_with("/conversations.json"))
            && !name.starts_with("__MACOSX/");
        if wanted && best.as_ref().is_none_or(|(len, _)| name.len() < *len) {
            best = Some((name.len(), entry));
        }
        true
    })?;
    Some(best.map(|(_, e)| e))
}

/// The first `max` entries' paths and sizes, and how many entries there are, read
/// from the central directory without inflating anything. None when the file has no
/// directory Chat can read.
pub(crate) fn listing(zip: &[u8], max: usize) -> Option<(u64, Vec<(String, u64)>)> {
    let mut entries = vec![];
    let count = walk(zip, |name, entry| {
        entries.push((String::from_utf8_lossy(name).into_owned(), entry.size));
        entries.len() < max
    })?;
    Some((count, entries))
}

/// Calls `each` with every entry of the central directory, in order, until it
/// returns false. Returns the directory's entry count.
fn walk(zip: &[u8], mut each: impl FnMut(&[u8], Entry) -> bool) -> Option<u64> {
    let tail = zip.len().saturating_sub(22 + 65_535);
    let end = (tail..zip.len().saturating_sub(21))
        .rev()
        .find(|&i| zip[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])?;
    let mut count = u64::from(u16_at(zip, end as u64 + 10).ok()?);
    let mut at = u64::from(u32_at(zip, end as u64 + 16).ok()?);
    // A zip64 end record holds the real count and offset.
    if end >= 20 && u32_at(zip, end as u64 - 20).ok()? == 0x0706_4b50 {
        let record = u64_at(zip, end as u64 - 12).ok()?;
        if u32_at(zip, record).ok()? != 0x0606_4b50 {
            return None;
        }
        count = u64_at(zip, record + 32).ok()?;
        at = u64_at(zip, record + 48).ok()?;
    }
    for _ in 0..count {
        if u32_at(zip, at).ok()? != 0x0201_4b50 {
            return None;
        }
        let method = u16_at(zip, at + 10).ok()?;
        let crc = u32_at(zip, at + 16).ok()?;
        let mut compressed = u64::from(u32_at(zip, at + 20).ok()?);
        let mut size = u64::from(u32_at(zip, at + 24).ok()?);
        let name_len = u64::from(u16_at(zip, at + 28).ok()?);
        let extra_len = u64::from(u16_at(zip, at + 30).ok()?);
        let comment_len = u64::from(u16_at(zip, at + 32).ok()?);
        let mut offset = u64::from(u32_at(zip, at + 42).ok()?);
        let name = slice(zip, at + 46, name_len)?;
        let mut extra = slice(zip, at + 46 + name_len, extra_len)?;
        while extra.len() >= 4 {
            let id = u16::from_le_bytes([extra[0], extra[1]]);
            let len = usize::from(u16::from_le_bytes([extra[2], extra[3]]));
            let body = extra.get(4..4 + len)?;
            if id == 1 {
                let mut values = body
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| u64::from_le_bytes(*c));
                for field in [&mut size, &mut compressed, &mut offset] {
                    if *field == u64::from(u32::MAX) {
                        *field = values.next()?;
                    }
                }
            }
            extra = &extra[4 + len..];
        }
        let entry = Entry {
            method,
            crc,
            compressed,
            size,
            offset,
        };
        if !each(name, entry) {
            break;
        }
        at += 46 + name_len + extra_len + comment_len;
    }
    Some(count)
}

fn slice(zip: &[u8], at: u64, len: u64) -> Option<&[u8]> {
    let start = usize::try_from(at).ok()?;
    zip.get(start..start.checked_add(usize::try_from(len).ok()?)?)
}
fn u16_at(zip: &[u8], at: u64) -> Result<u16, ApiError> {
    slice(zip, at, 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| ApiError::bad(BROKEN))
}
fn u32_at(zip: &[u8], at: u64) -> Result<u32, ApiError> {
    slice(zip, at, 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| ApiError::bad(BROKEN))
}
fn u64_at(zip: &[u8], at: u64) -> Result<u64, ApiError> {
    slice(zip, at, 8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| ApiError::bad(BROKEN))
}

/// Fails the read when the entry ends short or its checksum differs.
struct Checked<R> {
    inner: R,
    crc: crc32fast::Hasher,
    expected: u32,
    size: u64,
    read: u64,
    done: bool,
}
impl<R: Read> Read for Checked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.crc.update(&buf[..n]);
        self.read += n as u64;
        if n == 0 && !buf.is_empty() && !self.done {
            self.done = true;
            let crc = std::mem::take(&mut self.crc).finalize();
            if self.read != self.size || crc != self.expected {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "damaged zip entry",
                ));
            }
        }
        Ok(n)
    }
}
