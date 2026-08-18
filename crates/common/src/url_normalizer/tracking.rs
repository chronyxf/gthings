// ---------------------------------------------------------------------------
// Tracking-parameter stripping
// ---------------------------------------------------------------------------

/// Query-parameter keys that are considered tracking / marketing noise.
const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "fbclid",
    "gclid",
    "_ga",
    "_gl",
    "mc_cid",
    "mc_eid",
];

pub(super) fn is_tracking_param(key: &str) -> bool {
    TRACKING_PARAMS.contains(&key)
}

/// Build a percent-encoded query string from key-value pairs.
///
/// Uses [`url::form_urlencoded::Serializer`] for correct percent-encoding.
pub(super) fn build_query_string<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        serializer.append_pair(k.as_ref(), v.as_ref());
    }
    serializer.finish()
}
