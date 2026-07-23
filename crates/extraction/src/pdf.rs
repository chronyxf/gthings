/// PDF text extraction — pure Rust implementation.
///
///
/// Pipeline:
/// 1. Validate PDF magic number (`%PDF-`)
/// 2. Find stream objects with their filter metadata
/// 3. Decompress FlateDecode streams via `flate2`
/// 4. Extract parenthesized strings from content stream operators (Tj, TJ)
/// 5. Join and return the extracted text
///
/// No external PDF library is needed — this works with raw bytes and regex.
use flate2::read::ZlibDecoder;
use regex::Regex;
use std::io::Read;
use std::sync::OnceLock;

use common::GthingsError;

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
    /// # use extraction::PdfExtractor;
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

// Stream extraction

/// Extract all stream objects from raw PDF bytes.
///
/// Each PDF object is delimited by `N M obj` ... `endobj`.
/// Each stream object contains `stream` ... `endstream` with raw data.
/// `FlateDecode` filter is detected by scanning the object header.
fn extract_streams(bytes: &[u8]) -> Vec<PdfStream> {
    let mut streams: Vec<PdfStream> = Vec::new();
    let mut pos = 0;

    while let Some(stream_start) = find_pattern(bytes, b"stream", pos) {
        let data_begin = stream_start + 6; // skip "stream"
        // Skip whitespace/newline after "stream"
        let mut data_start = data_begin;
        while data_start < bytes.len() && is_pdf_whitespace(bytes[data_start]) {
            data_start += 1;
        }

        // Find "endstream"
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

                pos = end_pos + 9; // skip "endstream"
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
/// Scans backward from `stream_pos` to find the containing `obj` keyword and
/// returns the header text. Returns an empty string if no `obj` is found.
fn get_obj_header(bytes: &[u8], stream_pos: usize) -> String {
    // Scan backward from stream_pos to find "obj"
    let search_start = stream_pos.saturating_sub(500);
    let before = &bytes[search_start..stream_pos];

    if let Some(obj_pos) = before.windows(3).rposition(|w| w == b"obj") {
        let header = &before[obj_pos..];
        String::from_utf8_lossy(header).to_string()
    } else {
        String::new()
    }
}

// Decompression

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

// Text extraction from content streams

/// Extract text from a decompressed PDF content stream.
///
/// Uses PDF text operators:
/// - `Tj`: show string (parenthesized text followed by Tj)
/// - `TJ`: show with positioning array
/// - `'` : move to next line and show string
/// - `"` : set word/char spacing and show
///
/// PDF text in content streams is enclosed in parentheses with backslash escapes:
/// - `\(` → `(`, `\)` → `)`, `\\` → `\`
/// - `\ddd` → octal character code (e.g., `\050` → `(`)
/// - `\n` → newline (literal `\n` in the PDF source)
fn extract_text_from_stream(data: &[u8]) -> String {
    let src = String::from_utf8_lossy(data);
    let mut results: Vec<String> = Vec::new();

    // Match parenthesized strings in PDF content streams.
    // Handles basic nested parentheses by tracking depth.
    let chars: Vec<char> = src.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '(' {
            let mut depth = 1;
            let mut j = i + 1;
            let mut escaped = false;

            while j < len && depth > 0 {
                if escaped {
                    escaped = false;
                    j += 1;
                    continue;
                }
                if chars[j] == '\\' {
                    escaped = true;
                    j += 1;
                    continue;
                }
                if chars[j] == '(' {
                    depth += 1;
                } else if chars[j] == ')' {
                    depth -= 1;
                }
                j += 1;
            }

            let inner: String = chars[i + 1..j - 1].iter().collect();
            let unescaped = unescape_pdf_string(&inner);
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

/// Unescape a PDF string according to the PDF spec.
///
/// Handles:
/// - Octal escapes: `\ddd` (e.g., `\050` → `(`)
/// - Backslash escapes: `\(` → `(`, `\)` → `)`, `\\` → `\`
/// - Literal newlines: `\n` → newline
fn unescape_pdf_string(s: &str) -> String {
    static OCTAL_RE: OnceLock<Regex> = OnceLock::new();
    let octal_re = OCTAL_RE.get_or_init(|| Regex::new(r"\\([0-7]{3})").expect("valid regex"));

    // Step 1: replace octal escapes
    let s = octal_re.replace_all(s, |caps: &regex::Captures| {
        let oct = caps.get(1).map_or("", |m| m.as_str());
        let code = u32::from_str_radix(oct, 8).unwrap_or(0);
        char::from_u32(code).map_or_else(|| '\u{FFFD}'.to_string(), |c| c.to_string())
    });

    // Step 2: replace `\n` with actual newline
    static NL_RE: OnceLock<Regex> = OnceLock::new();
    let nl_re = NL_RE.get_or_init(|| Regex::new(r"\\n").expect("valid regex"));
    let s = nl_re.replace_all(&s, "\n");

    // Step 3: remove remaining backslash escapes (\, \(, \), etc.)
    static ESC_RE: OnceLock<Regex> = OnceLock::new();
    let esc_re = ESC_RE.get_or_init(|| Regex::new(r"\\(.)").expect("valid regex"));
    let s = esc_re.replace_all(&s, "$1");

    s.trim().to_string()
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
