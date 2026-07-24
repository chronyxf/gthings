/// PDF text extraction — pure Rust implementation.
///
/// Pipeline: validate magic number, find streams, decompress FlateDecode,
/// extract parenthesized strings from content operators, join text.
/// No external PDF library — raw bytes and regex.
use flate2::read::ZlibDecoder;
use regex::Regex;
use std::io::Read;
use std::sync::OnceLock;

use gthings_common::GthingsError;

/// Raw PDF stream data extracted from a PDF object.
struct PdfStream {
    /// Raw bytes of the stream content (compressed or uncompressed).
    data: Vec<u8>,
    /// Whether this stream uses FlateDecode compression.
    is_flate: bool,
    /// Object header text between `obj` and `stream` (for filter detection).
    obj_header: String,
}

/// PDF text extractor.
///
/// Provides methods for extracting text from PDF bytes, validating PDF format,
/// and counting pages.
///
/// # Examples
///
/// ```ignore
/// let pdf_bytes = std::fs::read("document.pdf").unwrap();
/// let text = PdfExtractor::extract(&pdf_bytes).unwrap();
/// println!("Extracted {} characters", text.len());
/// ```
pub struct PdfExtractor;

impl PdfExtractor {
    /// Extract text from PDF bytes.
    ///
    /// Returns the extracted text joined with spaces, or an error if:
    /// - The bytes are not a valid PDF (missing `%PDF-` magic number)
    /// - No stream objects are found
    /// - All streams fail to decompress or yield no text
    pub fn extract(bytes: &[u8]) -> Result<String, GthingsError> {
        if !Self::is_pdf(bytes) {
            return Err(GthingsError::Parse(
                "Not a PDF file (magic number mismatch — expected %PDF-*)".into(),
            ));
        }

        let streams = extract_streams(bytes);

        if streams.is_empty() {
            return Err(GthingsError::Parse("No stream objects found in PDF".into()));
        }

        let mut all_text_parts: Vec<String> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for stream in &streams {
            let decompressed = if stream.is_flate {
                match decompress_flate(&stream.data) {
                    Ok(d) => d,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                }
            } else {
                // Check for unsupported compression filters before treating as plain
                let header_upper = stream.obj_header.to_uppercase();
                let unsupported = [
                    "LZWDECODE",
                    "ASCII85DECODE",
                    "ASCIIHEXDECODE",
                    "RUNLENGTHDECODE",
                    "CCITTFAXDECODE",
                    "CCITT FAX DECODE",
                ];
                if let Some(_filter) = unsupported.iter().find(|&&f| header_upper.contains(f)) {
                    let preview_end = std::cmp::min(60, stream.obj_header.len());
                    errors.push(format!(
                        "Unsupported compression filter in stream: {}",
                        &stream.obj_header[..preview_end]
                    ));
                    continue;
                }
                stream.data.clone()
            };

            let text = extract_text_from_stream(&decompressed);
            if !text.is_empty() {
                all_text_parts.push(text);
            }
        }

        let result = all_text_parts.join(" ");
        let result = result.split_whitespace().collect::<Vec<_>>().join(" ");

        if result.is_empty() {
            let detail = if errors.is_empty() {
                "No text extracted from PDF streams (PDF may contain only images or non-text content)".into()
            } else {
                format!(
                    "No text extracted. {} stream(s) had errors: {}",
                    errors.len(),
                    errors[0]
                )
            };
            return Err(GthingsError::Parse(detail));
        }

        Ok(result)
    }

    /// Check if bytes are a valid PDF (starts with `%PDF-`).
    ///
    /// # Examples
    ///
    /// ```
    /// # use gthings_extraction::PdfExtractor;
    /// assert!(!PdfExtractor::is_pdf(b"not a pdf"));
    /// ```
    pub fn is_pdf(bytes: &[u8]) -> bool {
        bytes.len() >= 5 && bytes[..5] == *b"%PDF-"
    }

    /// Count pages in PDF by counting `/Type /Page` entries.
    ///
    /// This is a heuristic — accurate for well-formed PDFs.
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes cannot be decoded as a string.
    pub fn count_pages(bytes: &[u8]) -> Result<usize, GthingsError> {
        let src = String::from_utf8_lossy(bytes);
        let re =
            Regex::new(r"/Type\s*/Page[^s]").map_err(|e| GthingsError::Parse(e.to_string()))?;
        Ok(re.find_iter(&src).count())
    }
}

/// Extract all stream objects from raw PDF bytes.
///
/// Detects FlateDecode filter by scanning the object header.
fn extract_streams(bytes: &[u8]) -> Vec<PdfStream> {
    let mut streams: Vec<PdfStream> = Vec::new();
    let mut pos = 0;

    while let Some(stream_start) = find_pattern(bytes, b"stream", pos) {
        let mut data_start = stream_start + 6;
        while data_start < bytes.len() && is_pdf_whitespace(bytes[data_start]) {
            data_start += 1;
        }

        match find_pattern(bytes, b"endstream", data_start) {
            Some(end_pos) => {
                // Trim trailing whitespace before endstream
                let mut data_end = end_pos;
                while data_end > data_start && is_pdf_whitespace(bytes[data_end - 1]) {
                    data_end -= 1;
                }

                let raw_data = bytes[data_start..data_end].to_vec();

                // Get object header for filter detection (FlateDecode + unsupported)
                let obj_header = get_obj_header(bytes, stream_start);
                let is_flate = obj_header.to_uppercase().contains("FLATEDECODE");

                streams.push(PdfStream {
                    data: raw_data,
                    is_flate,
                    obj_header,
                });

                pos = end_pos + 9;
            }
            None => break,
        }
    }

    streams
}

/// Find a byte pattern starting from a given position.
fn find_pattern(bytes: &[u8], pattern: &[u8], start: usize) -> Option<usize> {
    if start >= bytes.len() {
        return None;
    }
    bytes[start..]
        .windows(pattern.len())
        .position(|w| w == pattern)
        .map(|p| start + p)
}

/// Check if a byte is PDF whitespace.
fn is_pdf_whitespace(b: u8) -> bool {
    b == b' ' || b == b'\n' || b == b'\r' || b == b'\t'
}

/// Get the object header text between `obj` and `stream` for filter detection.
///
/// Returns empty string if no `obj` is found.
fn get_obj_header(bytes: &[u8], stream_pos: usize) -> String {
    let search_start = stream_pos.saturating_sub(500);
    let before = &bytes[search_start..stream_pos];

    if let Some(obj_pos) = before.windows(3).rposition(|w| w == b"obj") {
        let header = &before[obj_pos..];
        String::from_utf8_lossy(header).to_string()
    } else {
        String::new()
    }
}

/// Decompress a FlateDecode (zlib) stream.
fn decompress_flate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = ZlibDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("incorrect header check") {
            "Corrupted FlateDecode stream: incorrect header check (data is not zlib-compressed)"
                .into()
        } else if msg.contains("invalid distance code") || msg.contains("invalid literal/length") {
            format!("Corrupted FlateDecode stream: {msg}")
        } else if msg.contains("unexpected end of data") {
            "Corrupted FlateDecode stream: unexpected end of data (stream may be truncated)".into()
        } else if msg.contains("invalid block type") || msg.contains("unknown compression method") {
            format!("Corrupted FlateDecode stream: {msg}")
        } else {
            format!("FlateDecode decompression error: {msg}")
        }
    })?;
    Ok(out)
}

/// Extract text from a decompressed PDF content stream.
///
/// Handles parenthesized text with backslash escapes and PDF text operators.
fn extract_text_from_stream(data: &[u8]) -> String {
    let mut results: Vec<String> = Vec::new();
    let len = data.len();
    let mut i = 0;

    // Scan bytes directly — avoid Vec<char> allocation and lossy UTF-8 conversion
    while i < len {
        if data[i] == b'(' {
            let mut depth = 1;
            let mut j = i + 1;

            while j < len && depth > 0 {
                if data[j] == b'\\' {
                    j += 2; // skip escaped character
                    continue;
                }
                if data[j] == b'(' {
                    depth += 1;
                } else if data[j] == b')' {
                    depth -= 1;
                }
                j += 1;
            }

            let inner = &data[i + 1..j - 1];
            let unescaped = unescape_pdf_string_bytes(inner);
            if !unescaped.is_empty() {
                results.push(unescaped);
            }

            i = j;
        } else {
            i += 1;
        }
    }

    results.join(" ")
}

/// Unescape a PDF string (octal, backslash escapes, and literal newlines).
fn unescape_pdf_string(s: &str) -> String {
    static ESC_RE: OnceLock<Regex> = OnceLock::new();
    let esc_re =
        ESC_RE.get_or_init(|| Regex::new(r"\\(?:([0-7]{3})|(n)|(.))").expect("valid regex"));
    let s = esc_re.replace_all(s, |caps: &regex::Captures| -> String {
        if let Some(oct) = caps.get(1) {
            let code = u32::from_str_radix(oct.as_str(), 8).unwrap_or(0);
            return char::from_u32(code).map_or_else(|| '\u{FFFD}'.to_string(), |c| c.to_string());
        }
        if caps.get(2).is_some() {
            return "\n".to_string();
        }
        // General escape: \X → X
        caps.get(3)
            .map_or(String::new(), |m| m.as_str().to_string())
    });
    s.trim().to_string()
}

/// Byte-level variant of `unescape_pdf_string`.
fn unescape_pdf_string_bytes(data: &[u8]) -> String {
    let s = std::str::from_utf8(data).unwrap_or("");
    unescape_pdf_string(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_pdf() {
        assert!(PdfExtractor::is_pdf(b"%PDF-1.4"));
        assert!(!PdfExtractor::is_pdf(b"not a pdf"));
        assert!(!PdfExtractor::is_pdf(b""));
        assert!(!PdfExtractor::is_pdf(b"%PD"));
    }

    #[test]
    fn test_extract_invalid_magic() {
        let result = PdfExtractor::extract(b"not a pdf");
        result.unwrap_err();
    }

    #[test]
    fn test_extract_no_streams() {
        // A minimal "PDF" with magic number but no streams
        let pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF";
        let result = PdfExtractor::extract(pdf);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No stream"));
    }

    #[test]
    fn test_count_pages() {
        let pdf =
            b"%PDF-1.4\n1 0 obj\n<< /Type /Page >>\nendobj\n2 0 obj\n<< /Type /Page >>\nendobj";
        let count = PdfExtractor::count_pages(pdf).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_unescape_pdf_string() {
        assert_eq!(unescape_pdf_string(r"Hello World"), "Hello World");
        assert_eq!(unescape_pdf_string(r"\(parentheses\)"), "(parentheses)");
        assert_eq!(unescape_pdf_string(r"\\backslash"), "\\backslash");
        assert_eq!(unescape_pdf_string(r"\050\051"), "()"); // octal for ( and )
        assert_eq!(unescape_pdf_string(r"line1\nline2"), "line1\nline2");
    }

    #[test]
    fn test_extract_text_from_stream() {
        // Simple content stream with Tj operator
        let content = b"(Hello World) Tj\n(PDF Text) Tj";
        let text = extract_text_from_stream(content);
        assert!(text.contains("Hello World"));
        assert!(text.contains("PDF Text"));
    }
}
