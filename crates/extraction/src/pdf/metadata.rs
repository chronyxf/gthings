/// Metadata extracted from a PDF document's /Info dictionary and XMP metadata.
#[derive(Debug, Clone, Default)]
pub(crate) struct PdfMetadata {
    pub author: Option<String>,
    pub title: Option<String>,
    pub creator: Option<String>,
    pub creation_date: Option<String>,
}

use regex::Regex;
use std::io::Read;

use crate::article::ExtractionError;

// ---------------------------------------------------------------------------
// PDF metadata extraction (regex-based, no parser module dependency)
// ---------------------------------------------------------------------------

/// Parse the PDF trailer to extract the /Info reference and /Root reference.
/// Returns `Ok(Some((info_obj_num, info_gen_num, root_obj_num)))` on success,
/// `Ok(None)` when the trailer or required references are not found,
/// or `Err(ExtractionError)` when the PDF bytes are not valid UTF-8.
fn parse_pdf_trailer(bytes: &[u8]) -> Result<Option<(u32, u32, u32)>, ExtractionError> {
    let content = std::str::from_utf8(bytes)
        .map_err(|e| ExtractionError::Parse(format!("non-UTF8 PDF content: {e}")))?;

    // Find "trailer" keyword — search from the end for efficiency
    let trailer_pos = match content.rfind("trailer") {
        Some(pos) => pos,
        None => return Ok(None),
    };
    let after_trailer = &content[trailer_pos..];

    // Find /Info reference pattern: /Info <obj> <gen> R
    let info_re = Regex::new(r"/Info\s+(\d+)\s+(\d+)\s+R")
        .expect("valid static regex: /Info reference in PDF trailer");
    // Find /Root reference pattern: /Root <obj> <gen> R
    let root_re = Regex::new(r"/Root\s+(\d+)\s+(\d+)\s+R")
        .expect("valid static regex: /Root reference in PDF trailer");

    let info_caps = match info_re.captures(after_trailer) {
        Some(caps) => caps,
        None => return Ok(None),
    };
    let root_caps = match root_re.captures(after_trailer) {
        Some(caps) => caps,
        None => return Ok(None),
    };

    let info_num = info_caps[1]
        .parse::<u32>()
        .map_err(|_| ExtractionError::Parse("PDF /Info object number is not a valid u32".into()))?;
    let info_gen = info_caps[2].parse::<u32>().map_err(|_| {
        ExtractionError::Parse("PDF /Info generation number is not a valid u32".into())
    })?;
    let root_num = root_caps[1]
        .parse::<u32>()
        .map_err(|_| ExtractionError::Parse("PDF /Root object number is not a valid u32".into()))?;

    Ok(Some((info_num, info_gen, root_num)))
}

/// Parse a PDF indirect object and return its raw content as a string.
/// Handles the pattern: `<obj> <gen> obj ... endobj`
fn parse_pdf_object(
    bytes: &[u8],
    obj_num: u32,
    gen_num: u32,
) -> Result<Option<String>, ExtractionError> {
    let content = std::str::from_utf8(bytes)
        .map_err(|e| ExtractionError::Parse(format!("non-UTF8 PDF content: {e}")))?;
    let pattern = format!(r"(?s){} {} obj(.*?)endobj", obj_num, gen_num);
    let re = Regex::new(&pattern)
        .map_err(|e| ExtractionError::Parse(format!("invalid PDF object regex: {e}")))?;
    let caps = match re.captures(content) {
        Some(caps) => caps,
        None => return Ok(None),
    };
    Ok(Some(caps[1].trim().to_string()))
}

/// Parse a PDF dictionary string (e.g. `/Author (Bram Moolenaar) /Title (Vim)`)
/// Extracts values for specified keys.
fn parse_pdf_dict_value(dict_content: &str, key: &str) -> Option<String> {
    // Try parenthesized strings first: /Key (value)
    let pattern = format!(r"(?s)/{}\s*\((.*?)\)", regex::escape(key));
    let re = match Regex::new(&pattern) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, key = %key, "invalid PDF dict regex");
            return None;
        }
    };
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
    let hex_re = match Regex::new(&hex_pattern) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, key = %key, "invalid PDF dict hex regex");
            return None;
        }
    };
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
pub(super) fn extract_pdf_info_metadata(
    bytes: &[u8],
) -> Result<Option<PdfMetadata>, ExtractionError> {
    let (info_num, info_gen, _) = match parse_pdf_trailer(bytes)? {
        Some(v) => v,
        None => return Ok(None),
    };
    let obj_content = match parse_pdf_object(bytes, info_num, info_gen)? {
        Some(v) => v,
        None => return Ok(None),
    };

    let to_opt = |val: Option<String>| val.filter(|s| !s.is_empty());

    Ok(Some(PdfMetadata {
        author: to_opt(parse_pdf_dict_value(&obj_content, "Author")),
        title: to_opt(parse_pdf_dict_value(&obj_content, "Title")),
        creator: to_opt(parse_pdf_dict_value(&obj_content, "Creator")),
        creation_date: to_opt(parse_pdf_dict_value(&obj_content, "CreationDate")),
    }))
}

/// Extract PDF creation date from XMP metadata or /Info dict.
/// Converts PDF date format (D:20240726123456+02'00') to ISO 8601,
/// preserving the UTC offset instead of hardcoding `Z`.
pub(super) fn normalize_pdf_date(date_str: &str) -> Option<String> {
    let date_str = date_str.trim();
    // PDF date format: D:YYYYMMDDHHmmSSOHH'mm' where O is the UTC offset
    // sign (`+`, `-`, or `Z`) and HH'mm' the offset hours/minutes.
    let re = Regex::new(
        r"^D:(\d{4})(\d{2})?(\d{2})?(\d{2})?(\d{2})?(\d{2})?([Z+-])?(\d{2})?'?(\d{2})?'?",
    )
    .expect("valid static regex: PDF date format D:YYYYMMDDHHmmSS");
    let caps = re.captures(date_str)?;

    let year = &caps[1];
    let month = caps.get(2).map_or("01", |m| m.as_str());
    let day = caps.get(3).map_or("01", |d| d.as_str());
    let hour = caps.get(4).map_or("00", |h| h.as_str());
    let min = caps.get(5).map_or("00", |m| m.as_str());
    let sec = caps.get(6).map_or("00", |s| s.as_str());

    let naive = chrono::NaiveDateTime::parse_from_str(
        &format!("{year}-{month}-{day} {hour}:{min}:{sec}"),
        "%Y-%m-%d %H:%M:%S",
    )
    .ok()?;

    // Preserve the PDF UTC offset rather than assuming `Z`.
    let offset_secs = match caps.get(7).map(|m| m.as_str()) {
        Some("+") => {
            let oh = caps
                .get(8)
                .map_or(0, |m| m.as_str().parse::<i32>().unwrap_or(0));
            let om = caps
                .get(9)
                .map_or(0, |m| m.as_str().parse::<i32>().unwrap_or(0));
            oh * 3600 + om * 60
        }
        Some("-") => {
            let oh = caps
                .get(8)
                .map_or(0, |m| m.as_str().parse::<i32>().unwrap_or(0));
            let om = caps
                .get(9)
                .map_or(0, |m| m.as_str().parse::<i32>().unwrap_or(0));
            -(oh * 3600 + om * 60)
        }
        _ => 0,
    };
    let offset = chrono::FixedOffset::east_opt(offset_secs)?;
    let dt = chrono::TimeZone::from_local_datetime(&offset, &naive).single()?;

    Some(dt.to_rfc3339())
}

/// Find and extract XMP metadata from a PDF file.
/// XMP is stored as a stream object referenced from the catalog's /Metadata entry.
pub(super) fn extract_pdf_xmp_metadata(
    bytes: &[u8],
) -> Result<Option<PdfMetadata>, ExtractionError> {
    // Find /Metadata reference in the catalog (root) object
    // First find the root object number from trailer
    let (_, _, root_num) = parse_pdf_trailer(bytes)?
        .ok_or_else(|| ExtractionError::Parse("no /Root ref in trailer for XMP lookup".into()))?;

    // Read the root object
    let root_content = parse_pdf_object(bytes, root_num, 0)?
        .ok_or_else(|| ExtractionError::Parse("root object not found for XMP lookup".into()))?;

    // Find /Metadata reference in root
    let meta_re = Regex::new(r"/Metadata\s+(\d+)\s+(\d+)\s+R")
        .expect("valid static regex: /Metadata reference in PDF catalog");
    let meta_caps = meta_re
        .captures(&root_content)
        .ok_or_else(|| ExtractionError::Parse("no /Metadata ref in root object".into()))?;
    let meta_num: u32 = meta_caps[1]
        .parse()
        .map_err(|e| ExtractionError::Parse(format!("XMP metadata object number parse: {e}")))?;
    let meta_gen: u32 = meta_caps[2].parse().map_err(|e| {
        ExtractionError::Parse(format!("XMP metadata generation number parse: {e}"))
    })?;

    // Read the metadata object
    let meta_obj = parse_pdf_object(bytes, meta_num, meta_gen)?
        .ok_or_else(|| ExtractionError::Parse("XMP metadata object not found".into()))?;

    // Extract the stream content
    let stream_re = Regex::new(r"(?s)stream\s(.+?)\s*endstream")
        .expect("valid static regex: stream/endstream content in PDF object");
    let stream_caps = stream_re
        .captures(&meta_obj)
        .ok_or_else(|| ExtractionError::Parse("no stream content in XMP metadata object".into()))?;
    let stream_data = stream_caps[1].as_bytes();

    // Try to decompress if FlateDecode
    let xml_data = if meta_obj.to_uppercase().contains("FLATEDECODE") {
        let mut decoder = flate2::read::ZlibDecoder::new(stream_data);
        let mut buf = Vec::new();
        decoder
            .read_to_end(&mut buf)
            .map_err(|e| ExtractionError::Parse(format!("failed to decompress XMP stream: {e}")))?;
        buf
    } else {
        stream_data.to_vec()
    };

    let xml_str = std::str::from_utf8(&xml_data)
        .map_err(|e| ExtractionError::Parse(format!("XMP stream is not valid UTF-8: {e}")))?;

    // Extract Dublin Core metadata from XMP
    let to_opt = |val: Option<String>| val.filter(|s| !s.is_empty());
    let author = extract_xmp_field(xml_str, "creator");
    let title = extract_xmp_field(xml_str, "title");
    let date = extract_xmp_field(xml_str, "date");

    Ok(Some(PdfMetadata {
        author: to_opt(author),
        title: to_opt(title),
        creator: None,
        creation_date: to_opt(date),
    }))
}

/// Extract a field value from XMP XML by namespace.
/// Handles: <dc:field>value</dc:field> and <dc:field><rdf:li>value</rdf:li></dc:field>
fn extract_xmp_field(xml: &str, field: &str) -> Option<String> {
    let escaped = regex::escape(field);

    // Try simple element: <dc:field>value</dc:field>
    let simple_re = Regex::new(&format!(r"<dc:{}[^>]*>([^<]+)</dc:{}>", escaped, escaped))
        .expect("valid regex: XMP simple field");
    if let Some(caps) = simple_re.captures(xml) {
        let val = caps[1].trim().to_string();
        if !val.is_empty() {
            return Some(val);
        }
    }

    // Try RDF container: <dc:field><rdf:li>value</rdf:li></dc:field>
    let rdf_re = Regex::new(&format!(
        r"<dc:{}[^>]*>\s*<rdf:li[^>]*>([^<]+)</rdf:li>\s*</dc:{}>",
        escaped, escaped,
    ))
    .expect("valid regex: XMP rdf field");
    if let Some(caps) = rdf_re.captures(xml) {
        let val = caps[1].trim().to_string();
        if !val.is_empty() {
            return Some(val);
        }
    }

    None
}
