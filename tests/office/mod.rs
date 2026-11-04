//! Office files built in code for the tests that upload them.

/// A zip archive holding each part uncompressed, as Office files are.
pub fn zip(parts: &[(&str, String)]) -> Vec<u8> {
    let crc = |bytes: &[u8]| {
        !bytes.iter().fold(!0u32, |c, b| {
            (0..8).fold(c ^ *b as u32, |c, _| {
                if c & 1 == 1 {
                    (c >> 1) ^ 0xedb8_8320
                } else {
                    c >> 1
                }
            })
        })
    };
    let header = |out: &mut Vec<u8>, name: &str, data: &[u8], central: Option<u32>| {
        let signature: u32 = if central.is_some() {
            0x0201_4b50
        } else {
            0x0403_4b50
        };
        out.extend(signature.to_le_bytes());
        if central.is_some() {
            out.extend(20u16.to_le_bytes());
        }
        out.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // version, flags, stored, time, date
        out.extend(crc(data).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        if let Some(offset) = central {
            out.extend([0u8; 10]);
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

/// A Word file with one paragraph per entry.
pub fn docx(paragraphs: &[String]) -> Vec<u8> {
    let body: String = paragraphs
        .iter()
        .map(|p| format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>"))
        .collect();
    zip(&[(
        "word/document.xml",
        format!("<w:document><w:body>{body}</w:body></w:document>"),
    )])
}
