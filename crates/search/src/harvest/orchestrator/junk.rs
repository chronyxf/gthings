//! Junk URL/title filtering.

/// Junk URL patterns to exclude from follow.
pub(crate) fn is_junk_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    // google.com/accounts/*, google.com/support/*, etc.
    if lower.starts_with("https://accounts.google.com/")
        || lower.starts_with("https://support.google.com/")
        || lower.starts_with("https://policies.google.com/")
    {
        return true;
    }
    // Generic junk patterns
    if lower.contains("/track?")
        || lower.contains("doubleclick.net")
        || lower.contains("googlesyndication.com")
    {
        return true;
    }
    // Generic sign-in / login / auth URLs
    if lower.contains("/signin")
        || lower.contains("/sign-in")
        || lower.contains("/login")
        || lower.contains("/log-in")
        || lower.contains("/auth/")
        || lower.contains("/auth?")
        || lower.ends_with("/auth")
    {
        return true;
    }
    false
}

/// Junk title patterns to exclude from follow (sign-in / login prompts).
pub(crate) fn is_junk_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    lower.contains("sign in to continue")
        || lower.contains("log in to continue")
        || lower.contains("sign in to read")
        || lower.contains("log in to read")
        || lower.contains("please sign in")
        || lower.contains("please log in")
        || lower.contains("sign in required")
        || lower.contains("log in required")
}
