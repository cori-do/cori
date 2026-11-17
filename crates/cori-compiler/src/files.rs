// SPDX-License-Identifier: Apache-2.0
//! Files a run takes (`file` and `files` parameters): the reference every step sees instead of the bytes, the limits a
//! parameter sets (`accept`, `max_size`, `max_files`), and the type of a file read from its first bytes. Shared by the
//! server (uploads, run parameters), the engine (what a model can read) and `cori run` (local files).
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A file as runs, steps, stores and fronts see it: never its bytes. The server's record of the file is the truth: a run's
/// parameters carry the reference it gives, whatever a caller sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoriFile {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub content_type: String,
    pub sha256: String,
}

impl CoriFile {
    pub fn to_json(&self) -> Value { serde_json::to_value(self).unwrap_or(Value::Null) }
}

/// What an id of a file looks like: `file_` and letters, digits, `_` or `-` (at most 80 characters in all).
pub fn valid_id(id: &str) -> bool {
    id.len() <= 80 && id.strip_prefix("file_").is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

/// The id a run parameter names for a file: the id itself (`"file_…"`) or a reference object (`{ "id": "file_…", … }`).
pub fn id_of(v: &Value) -> Option<&str> {
    match v { Value::String(s) => Some(s.as_str()), Value::Object(o) => o.get("id").and_then(Value::as_str), _ => None }
}

/// How many files a `files` parameter takes when it says nothing (`max_files`), and at most.
pub const MAX_FILES_DEFAULT: u64 = 20;
pub const MAX_FILES_LIMIT: u64 = 100;

/// A size as a workflow writes it: a number of bytes, or a number and a unit (`10MB`, `500 KB`, `2MiB`, `1.5GB`). KB, MB
/// and GB count by 1000, KiB, MiB and GiB by 1024.
pub fn parse_size(v: &Value) -> Result<u64, String> {
    if let Some(n) = v.as_u64() { return Ok(n); }
    let s = v.as_str().ok_or_else(|| format!("expected a size such as `10MB` or a number of bytes, got {v}"))?.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let (num, unit) = (s[..split].trim(), s[split..].trim().to_ascii_lowercase());
    let n: f64 = num.parse().map_err(|_| format!("`{s}` is not a size (a number then B, KB, MB, GB, KiB, MiB or GiB)"))?;
    let mult: f64 = match unit.as_str() {
        "" | "b" => 1.0,
        "kb" | "k" => 1e3, "mb" | "m" => 1e6, "gb" | "g" => 1e9,
        "kib" => 1024.0, "mib" => 1024.0 * 1024.0, "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return Err(format!("`{s}`: unknown unit `{unit}` (B, KB, MB, GB, KiB, MiB or GiB)")),
    };
    let bytes = n * mult;
    if !bytes.is_finite() || bytes < 1.0 { return Err(format!("`{s}` is not a size of at least one byte")); }
    Ok(bytes.round() as u64)
}

/// A size for people: `25 MiB`, `340 KiB`, `12 B`.
pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = n as f64; let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 { v /= 1024.0; u += 1; }
    if u == 0 { format!("{n} B") } else if v.fract().abs() < 0.05 { format!("{:.0} {}", v, UNITS[u]) } else { format!("{:.1} {}", v, UNITS[u]) }
}

/// Whether `entry` is something `accept` may list: a media type (`application/pdf`), a family (`image/*`) or an
/// extension (`.pdf`).
pub fn check_accept_entry(entry: &str) -> Result<(), String> {
    let e = entry.trim();
    let token = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_'));
    if let Some(ext) = e.strip_prefix('.') {
        if !ext.is_empty() && ext.len() <= 16 && ext.chars().all(|c| c.is_ascii_alphanumeric()) { return Ok(()); }
        return Err(format!("`{e}` is not an extension (a dot then letters or digits, such as `.pdf`)"));
    }
    match e.split_once('/') {
        Some((t, "*")) if token(t) => Ok(()),
        Some((t, s)) if token(t) && token(s) => Ok(()),
        _ => Err(format!("`{e}` is not a media type (`application/pdf`), a family (`image/*`) or an extension (`.pdf`)")),
    }
}

/// Whether a file named `name` of type `content_type` (as read from its bytes) is one that `accept` lets through: no
/// list lets every file through. An extension matches the name, and the type too when Cori knows the extension's type
/// (`.pdf` is a PDF, not anything named so).
pub fn accepts(accept: Option<&[String]>, name: &str, content_type: &str) -> bool {
    let Some(list) = accept else { return true };
    let ct = content_type.to_ascii_lowercase();
    let lname = name.to_ascii_lowercase();
    list.iter().any(|entry| {
        let e = entry.trim().to_ascii_lowercase();
        if e.starts_with('.') {
            if !lname.ends_with(&e) { return false; }
            return match type_of_extension(&e[1..]) { Some(t) => t == ct, None => true };
        }
        match e.split_once('/') {
            Some((family, "*")) => ct.split_once('/').map(|(f, _)| f == family).unwrap_or(false),
            _ => e == ct,
        }
    })
}

/// The media type Cori gives files with this extension (no dot, any case), when it knows one.
pub fn type_of_extension(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "text" | "log" => "text/plain",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "eml" => "message/rfc822",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "doc" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "ppt" => "application/vnd.ms-powerpoint",
        "msg" => "application/vnd.ms-outlook",
        _ => return None,
    })
}

fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// Text Cori knows by its extension: what a model reads as text.
fn is_text_type(t: &str) -> bool { t.starts_with("text/") || matches!(t, "application/json" | "application/xml" | "application/yaml" | "image/svg+xml" | "message/rfc822") }

/// A file's type, read from its first bytes (`head`, a few kilobytes are enough) and its name: never what a browser
/// said. Formats with a signature (PDF, images, archives, Office documents) by their signature; text (valid UTF-8 with no
/// NUL) by its extension, else `text/plain`; anything else is `application/octet-stream`.
pub fn sniff(head: &[u8], name: &str) -> String {
    let ext = extension(name);
    let by_ext = |default: &'static str, family: &[&str]| -> String {
        match ext.as_deref().and_then(type_of_extension) { Some(t) if family.contains(&t) => t.to_string(), _ => default.to_string() }
    };
    if head.starts_with(b"%PDF-") { return "application/pdf".into(); }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") { return "image/png".into(); }
    if head.starts_with(b"\xFF\xD8\xFF") { return "image/jpeg".into(); }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") { return "image/gif".into(); }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" { return "image/webp".into(); }
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        // a zip, or one of the formats made of one: by the extension, which only names a zip-based format here
        return by_ext("application/zip", &[
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation", "application/vnd.oasis.opendocument.text", "application/vnd.oasis.opendocument.spreadsheet",
        ]);
    }
    if head.starts_with(b"\x1f\x8b") { return "application/gzip".into(); }
    if head.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
        return by_ext("application/octet-stream", &["application/msword", "application/vnd.ms-excel", "application/vnd.ms-powerpoint", "application/vnd.ms-outlook"]);
    }
    if looks_like_text(head) {
        return match ext.as_deref().and_then(type_of_extension) { Some(t) if is_text_type(t) => t.to_string(), _ => "text/plain".to_string() };
    }
    "application/octet-stream".into()
}

/// UTF-8 (its BOM allowed, a character cut at the end of `head` allowed) without NUL: text.
fn looks_like_text(head: &[u8]) -> bool {
    if head.is_empty() { return true; }
    let b = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    if b.contains(&0) { return false; }
    match std::str::from_utf8(b) { Ok(_) => true, Err(e) => e.error_len().is_none() && b.len() - e.valid_up_to() < 4 }
}

/// A file's name as people see it: the last part of whatever path a browser sent, without control characters, at most
/// 200 characters; `file` when nothing is left.
pub fn clean_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last.chars().filter(|c| !c.is_control()).collect::<String>().trim().trim_matches('.').trim().to_string();
    let cut: String = if cleaned.chars().count() > 200 {
        // keep the extension
        match extension(&cleaned) { Some(ext) if ext.len() < 16 => { let stem: String = cleaned.chars().take(200 - ext.len() - 1).collect(); format!("{stem}.{ext}") } _ => cleaned.chars().take(200).collect() }
    } else { cleaned };
    if cut.is_empty() { "file".into() } else { cut }
}

/// Hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String { use sha2::Digest; hex::encode(sha2::Sha256::digest(bytes)) }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sizes_read_as_written() {
        assert_eq!(parse_size(&json!(1024)), Ok(1024));
        assert_eq!(parse_size(&json!("10MB")), Ok(10_000_000));
        assert_eq!(parse_size(&json!("500 KB")), Ok(500_000));
        assert_eq!(parse_size(&json!("2MiB")), Ok(2 * 1024 * 1024));
        assert_eq!(parse_size(&json!("1.5GB")), Ok(1_500_000_000));
        assert_eq!(parse_size(&json!("12")), Ok(12));
        for bad in [json!("ten MB"), json!("10XB"), json!("0"), json!(-3), json!(true)] { assert!(parse_size(&bad).is_err(), "{bad}"); }
        assert_eq!(human_size(25 * 1024 * 1024), "25 MiB");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(12), "12 B");
    }

    #[test]
    fn accept_lists_media_types_families_and_extensions() {
        for ok in ["application/pdf", "image/*", ".pdf", "application/vnd.ms-excel", ".tar"] { assert!(check_accept_entry(ok).is_ok(), "{ok}"); }
        for bad in ["pdf", "*/*x", ".", "image/", "/pdf", ".p d f"] { assert!(check_accept_entry(bad).is_err(), "{bad}"); }
        let a = vec!["application/pdf".to_string(), "image/*".to_string(), ".csv".to_string(), ".dwg".to_string()];
        assert!(accepts(Some(&a), "x.pdf", "application/pdf"));
        assert!(accepts(Some(&a), "scan.jpeg", "image/jpeg"));
        assert!(accepts(Some(&a), "rows.CSV", "text/csv"));
        // an extension Cori knows needs the type it names: a zip named .csv is no CSV
        assert!(!accepts(Some(&a), "rows.csv", "application/zip"));
        // an extension Cori does not know: the name decides
        assert!(accepts(Some(&a), "plan.dwg", "application/octet-stream"));
        assert!(!accepts(Some(&a), "notes.txt", "text/plain"));
        assert!(accepts(None, "anything.bin", "application/octet-stream"));
    }

    #[test]
    fn a_files_type_comes_from_its_bytes() {
        assert_eq!(sniff(b"%PDF-1.7\n...", "invoice.txt"), "application/pdf");
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n....", "a.pdf"), "image/png");
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0", "a"), "image/jpeg");
        assert_eq!(sniff(b"RIFF\x00\x00\x00\x00WEBPVP8 ", "a.webp"), "image/webp");
        assert_eq!(sniff(b"PK\x03\x04....", "report.docx"), "application/vnd.openxmlformats-officedocument.wordprocessingml.document");
        assert_eq!(sniff(b"PK\x03\x04....", "invoices.zip"), "application/zip");
        assert_eq!(sniff(b"PK\x03\x04....", "invoices.pdf"), "application/zip");
        assert_eq!(sniff(b"a,b\n1,2\n", "rows.csv"), "text/csv");
        assert_eq!(sniff("é,ü\n".as_bytes(), "rows.csv"), "text/csv");
        assert_eq!(sniff(b"{\"a\":1}", "x.json"), "application/json");
        assert_eq!(sniff(b"hello", "x.exe"), "text/plain");
        assert_eq!(sniff(b"<html><script>", "page.html"), "text/html");
        assert_eq!(sniff(b"MZ\x90\x00\x03\x00", "x.pdf"), "application/octet-stream");
        // a multi-byte character cut by the end of the head is still text
        assert_eq!(sniff(&"aé".as_bytes()[..2], "x.txt"), "text/plain");
    }

    #[test]
    fn names_and_ids() {
        assert_eq!(clean_name("C:\\Users\\me\\Invoice 12.pdf"), "Invoice 12.pdf");
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("bad\u{0}\u{7}name.pdf"), "badname.pdf");
        assert_eq!(clean_name("   "), "file");
        assert_eq!(clean_name("..."), "file");
        let long = format!("{}.pdf", "a".repeat(300));
        let c = clean_name(&long);
        assert!(c.ends_with(".pdf") && c.chars().count() == 200, "{c}");
        assert!(valid_id("file_0f3a9c"));
        for bad in ["file_", "files_x", "file_a/b", "x", &format!("file_{}", "a".repeat(80))] { assert!(!valid_id(bad), "{bad}"); }
        assert_eq!(id_of(&json!("file_1")), Some("file_1"));
        assert_eq!(id_of(&json!({ "id": "file_2", "name": "x" })), Some("file_2"));
        assert_eq!(id_of(&json!(3)), None);
    }
}
