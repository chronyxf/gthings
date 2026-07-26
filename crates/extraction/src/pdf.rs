/// PDF metadata extraction and PdfExtractor API.
///
/// Extracts text via the `pdftotext` CLI (poppler-utils).  No pure-Rust PDF
/// parser fallback — if pdftotext is unavailable or fails the extraction
/// returns an `ExtractionError::Empty`.
use regex::Regex;
use std::io::{Read, Write};
use std::process::Command;
use std::time::Instant;

use crate::article::{
    Article, ContentTree, ContinuationSignals, ExtractionError, ExtractionInfo, ExtractionMethod,
    QualityScore, SourceInfo,
};
use crate::extractor::Extractor;
use async_trait::async_trait;

/// Metadata extracted from a PDF document's /Info dictionary and XMP metadata.
#[derive(Debug, Clone, Default)]
pub struct PdfMetadata {
    pub author: Option<String>,
    pub title: Option<String>,
    pub subject: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub creation_date: Option<String>,
    pub mod_date: Option<String>,
}

/// PDF text extractor that delegates to `pdftotext`.
///
/// # Examples
///
/// ```ignore
/// let pdf_bytes = std::fs::read("document.pdf").unwrap();
/// let extractor = PdfExtractor;
/// let article = extractor.extract_article("https://example.com/doc.pdf", &pdf_bytes).unwrap();
/// ```
pub struct PdfExtractor;

impl PdfExtractor {
    /// Extract PDF content as an Article.
    ///
    /// Tries `pdftotext` first; if it fails or is unavailable the extraction
    /// is considered empty and quality is set to 0.
    pub fn extract_article(&self, url: &str, bytes: &[u8]) -> Result<Article, ExtractionError> {
        let start = Instant::now();

        if !Self::is_pdf(bytes) {
            return Err(ExtractionError::Parse("not a valid PDF".into()));
        }

        // Try pdftotext only — no pure-Rust fallback
        let text = match Self::try_pdftotext(bytes) {
            Some(t) => t,
            None => {
                return Err(ExtractionError::Empty(
                    "pdftotext failed or not available; no fallback parser".into(),
                ));
            }
        };

        let pages = Self::count_pages(bytes);

        let total_length = text.len();
        let duration_ms = start.elapsed().as_millis() as u64;

        // Extract PDF metadata
        let pdf_meta = extract_pdf_info_metadata(bytes).or_else(|| extract_pdf_xmp_metadata(bytes));

        let author = pdf_meta.as_ref().and_then(|m| m.author.clone());
        let published = pdf_meta.as_ref().and_then(|m| {
            m.creation_date
                .as_ref()
                .and_then(|d| normalize_pdf_date(d))
                .or_else(|| m.creation_date.clone())
        });
        let title = pdf_meta.as_ref().and_then(|m| m.title.clone());
        let source_site = pdf_meta
            .as_ref()
            .and_then(|m| m.creator.clone())
            .unwrap_or_default();

        let q_score = compute_readability_score(&text, total_length);
        let mut quality_reasons = Vec::new();
        if total_length <= 500 {
            quality_reasons.push("too_short".into());
        }
        if q_score <= 0.3 && total_length > 500 {
            quality_reasons.push("readability_artifacts".into());
        }
        let quality = QualityScore {
            score: crate::article::round_score(q_score),
            is_ok: q_score >= 0.5,
            reasons: quality_reasons,
        };

        Ok(Article {
            url: url.to_string(),
            title: title.unwrap_or_default(),
            source: SourceInfo {
                author,
                published,
                site_name: source_site,
                domain_authority: crate::article::round_score(
                    crate::extractor::compute_domain_authority(url),
                ),
                language: None,
            },
            extraction: ExtractionInfo {
                method: ExtractionMethod::PdfText,
                confidence: quality.score,
                accessed_at: chrono::Utc::now().to_rfc3339(),
                duration_ms,
            },
            body: ContentTree::Pdf {
                pages,
                text,
                has_toc: false,
            },
            signals: ContinuationSignals {
                truncated: false,
                total_length,
                returned_length: total_length,
                is_paywall: false,
                is_bot_blocked: false,
                is_empty_shell: total_length < 200,
                related_urls: Vec::new(),
            },
            quality,
        })
    }

    /// Check if bytes start with the PDF magic number `%PDF-`.
    fn is_pdf(bytes: &[u8]) -> bool {
        bytes.len() >= 5 && bytes[..5] == *b"%PDF-"
    }

    /// Count pages in PDF by counting `/Type /Page` entries (heuristic).
    fn count_pages(bytes: &[u8]) -> usize {
        let src = String::from_utf8_lossy(bytes);
        Regex::new(r"/Type\s*/Page[^s]")
            .ok()
            .map(|re| re.find_iter(&src).count())
            .unwrap_or(0)
    }

    /// Try extracting text via the `pdftotext` CLI tool (part of poppler-utils).
    ///
    /// Reads PDF bytes on stdin, writes plain text to stdout with proper word
    /// spacing. Handles TeX-produced and other PDFs where text is positioned
    /// entirely via the text matrix.
    ///
    /// Returns `None` when pdftotext is not installed, the subprocess fails,
    /// or the output is empty.
    fn try_pdftotext(bytes: &[u8]) -> Option<String> {
        let mut child = Command::new("pdftotext")
            .args(["-", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;

        // Pipe PDF bytes to stdin, then close it
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(bytes);
        }

        let output = child.wait_with_output().ok()?;
        if !output.status.success() {
            return None;
        }

        let text = std::str::from_utf8(&output.stdout).ok()?;
        let text = text.replace('\x0c', "\n"); // replace form-feeds with newlines
        let text = text.trim().to_string();

        if text.is_empty() { None } else { Some(text) }
    }
}

/// Compute readability-based quality score from extracted text.
///
/// Detects letter-spacing artifacts (e.g. "P r o c e e d i n g s") that
/// produce garbled text yet pass a pure-length heuristic.
fn compute_readability_score(text: &str, total_length: usize) -> f64 {
    let words: Vec<&str> = text.split_whitespace().collect();
    let word_count = words.len();

    if word_count == 0 {
        return 0.3;
    }

    // Average word length
    let total_chars: usize = words.iter().map(|w| w.len()).sum();
    let avg_word_length = total_chars as f64 / word_count as f64;

    // Percentage of single-letter "words"
    let single_letter_count = words.iter().filter(|w| w.len() == 1).count();
    let single_letter_pct = single_letter_count as f64 / word_count as f64;

    // Letter-spacing merged into one long string (no whitespace between chars)
    if avg_word_length > 20.0 {
        return 0.3;
    }

    // Unmerged letter-spacing: mostly single-char "words"
    if avg_word_length < 2.0 {
        return 0.3;
    }

    // Too many single-letter words (indicates spacing artifacts)
    if single_letter_pct > 0.40 {
        return 0.3;
    }

    // Length heuristic with punctuation boost
    if total_length > 500 {
        // Boost to 0.9 if text contains reasonable punctuation
        if text.contains('.') || text.contains(',') || text.contains(';') {
            0.9
        } else {
            0.8
        }
    } else {
        0.3
    }
}

#[async_trait]
impl Extractor for PdfExtractor {
    type Input = (String, Vec<u8>);

    async fn extract(&self, input: (String, Vec<u8>)) -> Result<Article, ExtractionError> {
        let (url, bytes) = input;
        self.extract_article(&url, &bytes)
    }

    fn method(&self) -> ExtractionMethod {
        ExtractionMethod::PdfText
    }
}

// ---------------------------------------------------------------------------
// PDF metadata extraction (regex-based, no parser module dependency)
// ---------------------------------------------------------------------------

/// Parse the PDF trailer to extract the /Info reference and /Root reference.
/// Returns (info_obj_num, info_gen_num, root_obj_num) or None.
fn parse_pdf_trailer(bytes: &[u8]) -> Option<(u32, u32, u32)> {
    let content = std::str::from_utf8(bytes).ok()?;

    // Find "trailer" keyword — search from the end for efficiency
    let trailer_pos = content.rfind("trailer")?;
    let after_trailer = &content[trailer_pos..];

    // Find /Info reference pattern: /Info <obj> <gen> R
    let info_re = Regex::new(r"/Info\s+(\d+)\s+(\d+)\s+R").ok()?;
    // Find /Root reference pattern: /Root <obj> <gen> R
    let root_re = Regex::new(r"/Root\s+(\d+)\s+(\d+)\s+R").ok()?;

    let info_caps = info_re.captures(after_trailer)?;
    let root_caps = root_re.captures(after_trailer)?;

    let info_num = info_caps[1].parse::<u32>().ok()?;
    let info_gen = info_caps[2].parse::<u32>().ok()?;
    let root_num = root_caps[1].parse::<u32>().ok()?;

    Some((info_num, info_gen, root_num))
}

/// Parse a PDF indirect object and return its raw content as a string.
/// Handles the pattern: `<obj> <gen> obj ... endobj`
fn parse_pdf_object(bytes: &[u8], obj_num: u32, gen_num: u32) -> Option<String> {
    let content = std::str::from_utf8(bytes).ok()?;
    let pattern = format!(r"(?s){} {} obj(.*?)endobj", obj_num, gen_num);
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(content)?;
    Some(caps[1].trim().to_string())
}

/// Parse a PDF dictionary string (e.g. `/Author (Bram Moolenaar) /Title (Vim)`)
/// Extracts values for specified keys.
fn parse_pdf_dict_value(dict_content: &str, key: &str) -> Option<String> {
    // Try parenthesized strings first: /Key (value)
    let pattern = format!(r"(?s)/{}\s*\((.*?)\)", regex::escape(key));
    let re = Regex::new(&pattern).ok()?;
    if let Some(caps) = re.captures(dict_content) {
        let val = caps[1].to_string();
        // Unescape PDF string escapes
        let val = val
            .replace(r"\n", "\n")
            .replace(r"\r", "\r")
            .replace(r"\t", "\t")
            .replace(r"\(", "(")
            .replace(r"\)", ")")
            .replace(r"\\", "\\");
        return Some(val);
    }

    // Try hex strings: /Key <hex>
    let hex_pattern = format!(r"/{}\s*<([0-9a-fA-F]*)>", regex::escape(key));
    let hex_re = Regex::new(&hex_pattern).ok()?;
    if let Some(caps) = hex_re.captures(dict_content) {
        let hex = &caps[1];
        if !hex.is_empty() {
            let bytes: Vec<u8> = (0..hex.len())
                .step_by(2)
                .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                .collect();
            return Some(String::from_utf8_lossy(&bytes).to_string());
        }
    }

    None
}

/// Extract metadata from PDF /Info dictionary by finding the object and parsing it.
fn extract_pdf_info_metadata(bytes: &[u8]) -> Option<PdfMetadata> {
    let (info_num, info_gen, _root_num) = parse_pdf_trailer(bytes)?;
    let obj_content = parse_pdf_object(bytes, info_num, info_gen)?;

    let to_opt = |val: Option<String>| val.filter(|s| !s.is_empty());

    Some(PdfMetadata {
        author: to_opt(parse_pdf_dict_value(&obj_content, "Author")),
        title: to_opt(parse_pdf_dict_value(&obj_content, "Title")),
        subject: to_opt(parse_pdf_dict_value(&obj_content, "Subject")),
        creator: to_opt(parse_pdf_dict_value(&obj_content, "Creator")),
        producer: to_opt(parse_pdf_dict_value(&obj_content, "Producer")),
        creation_date: to_opt(parse_pdf_dict_value(&obj_content, "CreationDate")),
        mod_date: to_opt(parse_pdf_dict_value(&obj_content, "ModDate")),
    })
}

/// Extract PDF creation date from XMP metadata or /Info dict.
/// Converts PDF date format (D:20240726123456+02'00') to ISO 8601.
fn normalize_pdf_date(date_str: &str) -> Option<String> {
    let date_str = date_str.trim();
    // PDF date format: D:YYYYMMDDHHmmSSOHH'mm'
    let re = Regex::new(r"^D:(\d{4})(\d{2})?(\d{2})?(\d{2})?(\d{2})?(\d{2})?").ok()?;
    let caps = re.captures(date_str)?;

    let year = &caps[1];
    let month = caps.get(2).map_or("01", |m| m.as_str());
    let day = caps.get(3).map_or("01", |d| d.as_str());
    let hour = caps.get(4).map_or("00", |h| h.as_str());
    let min = caps.get(5).map_or("00", |m| m.as_str());
    let sec = caps.get(6).map_or("00", |s| s.as_str());

    Some(format!(
        "{}-{}-{}T{}:{}:{}Z",
        year, month, day, hour, min, sec
    ))
}

/// Find and extract XMP metadata from a PDF file.
/// XMP is stored as a stream object referenced from the catalog's /Metadata entry.
fn extract_pdf_xmp_metadata(bytes: &[u8]) -> Option<PdfMetadata> {
    // Find /Metadata reference in the catalog (root) object
    // First find the root object number from trailer
    let (_, _, root_num) = parse_pdf_trailer(bytes)?;

    // Read the root object
    let root_content = parse_pdf_object(bytes, root_num, 0)?;

    // Find /Metadata reference in root
    let meta_re = Regex::new(r"/Metadata\s+(\d+)\s+(\d+)\s+R").ok()?;
    let meta_caps = meta_re.captures(&root_content)?;
    let meta_num: u32 = meta_caps[1].parse().ok()?;
    let meta_gen: u32 = meta_caps[2].parse().ok()?;

    // Read the metadata object
    let meta_obj = parse_pdf_object(bytes, meta_num, meta_gen)?;

    // Extract the stream content
    let stream_re = Regex::new(r"(?s)stream\s(.+?)\s*endstream").ok()?;
    let stream_caps = stream_re.captures(&meta_obj)?;
    let stream_data = stream_caps[1].as_bytes();

    // Try to decompress if FlateDecode
    let xml_data = if meta_obj.to_uppercase().contains("FLATEDECODE") {
        let mut decoder = flate2::read::ZlibDecoder::new(stream_data);
        let mut buf = Vec::new();
        decoder.read_to_end(&mut buf).ok()?;
        buf
    } else {
        stream_data.to_vec()
    };

    let xml_str = std::str::from_utf8(&xml_data).ok()?;

    // Extract Dublin Core metadata from XMP
    let author = extract_xmp_field(xml_str, "creator");
    let title = extract_xmp_field(xml_str, "title");
    let subject = extract_xmp_field(xml_str, "description");
    let date = extract_xmp_field(xml_str, "date");

    Some(PdfMetadata {
        author: author.filter(|s| !s.is_empty()),
        title: title.filter(|s| !s.is_empty()),
        subject: subject.filter(|s| !s.is_empty()),
        creator: None,
        producer: None,
        creation_date: date.filter(|s| !s.is_empty()),
        mod_date: None,
    })
}

/// Extract a field value from XMP XML by namespace.
/// Handles: <dc:field>value</dc:field> and <dc:field><rdf:li>value</rdf:li></dc:field>
fn extract_xmp_field(xml: &str, field: &str) -> Option<String> {
    // Try simple element: <dc:field>value</dc:field>
    let simple_re = Regex::new(&format!(
        r"<dc:{}[^>]*>([^<]+)</dc:{}>",
        regex::escape(field),
        regex::escape(field)
    ))
    .ok()?;
    if let Some(caps) = simple_re.captures(xml) {
        let val = caps[1].trim().to_string();
        if !val.is_empty() {
            return Some(val);
        }
    }

    // Try RDF container: <dc:field><rdf:li>value</rdf:li></dc:field>
    let rdf_re = Regex::new(&format!(
        r"<dc:{}[^>]*>\s*<rdf:li[^>]*>([^<]+)</rdf:li>\s*</dc:{}>",
        regex::escape(field),
        regex::escape(field)
    ))
    .ok()?;
    if let Some(caps) = rdf_re.captures(xml) {
        let val = caps[1].trim().to_string();
        if !val.is_empty() {
            return Some(val);
        }
    }

    None
}
