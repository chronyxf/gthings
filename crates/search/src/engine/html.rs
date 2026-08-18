//! Shared HTML/XML text-processing helpers used by the HTTP-based search
//! backends (brave, bing). Centralizing these avoids duplicating entity
//! decoding, tag stripping, and whitespace collapsing across engines.

/// Decodes named (`&amp;`, `&lt;`, ...) and numeric (`&#39;`, `&#x27;`)
/// HTML/XML entities in `s`. Unknown or malformed entities are kept verbatim.
///
/// Results are pushed directly into a single output buffer; no per-entity
/// intermediate `String` is allocated.
pub(crate) fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(idx) = rest.find('&') {
        out.push_str(&rest[..idx]);
        let after = &rest[idx..];
        let semicolon = after.find(';');
        let decoded = semicolon.and_then(|end| {
            let entity = &after[1..end];
            match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                "copy" => Some('\u{00a9}'),
                "reg" => Some('\u{00ae}'),
                "trade" => Some('\u{2122}'),
                "hellip" => Some('\u{2026}'),
                "mdash" => Some('\u{2014}'),
                "ndash" => Some('\u{2013}'),
                "rsquo" => Some('\u{2019}'),
                "lsquo" => Some('\u{2018}'),
                "ldquo" => Some('\u{201c}'),
                "rdquo" => Some('\u{201d}'),
                "middot" => Some('\u{00b7}'),
                "bull" => Some('\u{2022}'),
                "eacute" => Some('\u{00e9}'),
                "egrave" => Some('\u{00e8}'),
                "agrave" => Some('\u{00e0}'),
                "ccedil" => Some('\u{00e7}'),
                "uuml" => Some('\u{00fc}'),
                "ouml" => Some('\u{00f6}'),
                "auml" => Some('\u{00e4}'),
                "szlig" => Some('\u{00df}'),
                "deg" => Some('\u{00b0}'),
                "plusmn" => Some('\u{00b1}'),
                "times" => Some('\u{00d7}'),
                "divide" => Some('\u{00f7}'),
                "laquo" => Some('\u{00ab}'),
                "raquo" => Some('\u{00bb}'),
                "cent" => Some('\u{00a2}'),
                "pound" => Some('\u{00a3}'),
                "euro" => Some('\u{20ac}'),
                "yen" => Some('\u{00a5}'),
                _ => {
                    if let Some(hex) = entity
                        .strip_prefix("#x")
                        .or_else(|| entity.strip_prefix("#X"))
                    {
                        u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                    } else if let Some(dec) = entity.strip_prefix('#') {
                        dec.parse::<u32>().ok().and_then(char::from_u32)
                    } else {
                        None
                    }
                }
            }
        });
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &after[semicolon.map_or(1, |end| end + 1)..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Removes HTML/XML tags from `s`, leaving the text content intact (entities
/// are left as-is; call [`decode_entities`] separately if needed).
///
/// Only well-formed tags are stripped: a `<` is treated as a tag start only
/// when followed by a letter, `/`, `!`, or `?`. A bare `<` (e.g. in `<3`)
/// is kept verbatim.
///
/// A tag start that never finds a closing `>` (malformed/unterminated HTML)
/// is bounded: the scan stops after [`MAX_TAG_LEN`] characters or at a
/// newline, and the remainder is pushed back as text rather than silently
/// dropping the rest of the input.
pub(crate) fn strip_tags(s: &str) -> String {
    /// Upper bound on a single tag's length. Real tags are short; anything
    /// longer is treated as malformed and the scan is abandoned.
    const MAX_TAG_LEN: usize = 1024;

    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            match chars.peek() {
                Some(&n) if n.is_ascii_alphabetic() || n == '/' || n == '!' || n == '?' => {
                    let mut scanned = String::new();
                    let mut terminated = false;
                    for c2 in chars.by_ref() {
                        scanned.push(c2);
                        if c2 == '>' {
                            terminated = true;
                            break;
                        }
                        if c2 == '\n' || scanned.chars().count() >= MAX_TAG_LEN {
                            break;
                        }
                    }
                    if !terminated {
                        // Unterminated tag: keep the `<` and everything
                        // scanned so far as literal text instead of dropping
                        // the rest of the input.
                        out.push('<');
                        out.push_str(&scanned);
                    }
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Collapses runs of ANY whitespace (including newlines) into single spaces,
/// trimming leading and trailing whitespace — equivalent to
/// `split_whitespace().join(" ")` but in a single pass with no intermediate
/// `Vec`.
///
/// Note: this differs from `follow::clean::collapse_whitespace`, which
/// preserves newlines as paragraph breaks.
pub(crate) fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        }
    }
    out
}

/// True when `body` looks like a block page (Turnstile interstitial, consent
/// wall, or generic challenge) rather than a results page. Shared by the
/// Brave and Bing backends to detect CAPTCHA/challenge responses.
pub(crate) fn body_has_block_markers(body: &str) -> bool {
    let lower = body.to_lowercase();
    const MARKERS: [&str; 5] = ["turnstile", "cf-chl", "consent", "challenge", "captcha"];
    MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_named_entities() {
        assert_eq!(
            decode_entities("A &amp; B &lt;3 &gt;1 &quot;q&quot; &apos;a&apos; &nbsp;x"),
            "A & B <3 >1 \"q\" 'a'  x"
        );
    }

    #[test]
    fn decodes_common_named_entities() {
        assert_eq!(
            decode_entities(
                "&copy; &reg; &hellip; &mdash; &ndash; &rsquo; &ldquo; &rdquo; \
                 &middot; &eacute; &euro; &trade; &bull; &deg; &times;"
            ),
            "\u{00a9} \u{00ae} \u{2026} \u{2014} \u{2013} \u{2019} \u{201c} \u{201d} \
             \u{00b7} \u{00e9} \u{20ac} \u{2122} \u{2022} \u{00b0} \u{00d7}"
        );
    }

    #[test]
    fn decodes_numeric_entities() {
        assert_eq!(decode_entities("&#39; &#x27; &#65; &#x41;"), "' ' A A");
    }

    #[test]
    fn keeps_unknown_entities_verbatim() {
        assert_eq!(decode_entities("a &unknown; &"), "a &unknown; &");
    }

    #[test]
    fn strips_tags() {
        assert_eq!(
            strip_tags("<b>bold</b> and <i>italic</i>"),
            "bold and italic"
        );
        assert_eq!(strip_tags("<div class=\"x\">text</div>"), "text");
        assert_eq!(strip_tags("no tags"), "no tags");
    }

    #[test]
    fn strip_tags_handles_unterminated_tag() {
        // A tag start that never finds a closing '>' must not consume the
        // rest of the input; the remainder is preserved as literal text.
        assert_eq!(
            strip_tags("before <div class=\"x\" after"),
            "before <div class=\"x\" after"
        );
        // Unterminated tag bounded at a newline.
        assert_eq!(strip_tags("a <span\nb"), "a <span\nb");
        // Well-formed tags still stripped after an unterminated one.
        assert_eq!(strip_tags("<b>ok</b> <div"), "ok <div");
    }

    #[test]
    fn collapses_whitespace() {
        assert_eq!(collapse_whitespace("  spaced\n\t out  "), "spaced out");
        assert_eq!(collapse_whitespace("a   b"), "a b");
        assert_eq!(collapse_whitespace("  leading"), "leading");
        assert_eq!(collapse_whitespace("trailing  "), "trailing");
        assert_eq!(collapse_whitespace(""), "");
    }
}
