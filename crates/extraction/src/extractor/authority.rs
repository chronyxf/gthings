/// Compute a domain authority score (0.0-1.0) for a hostname.
///
/// Uses a curated list of academic, technical, news, and government domains.
/// Unknown domains default to 0.5. The score helps AI agents assess source trustworthiness.
pub fn domain_authority(host: &str) -> f64 {
    let domain = host.to_lowercase();

    // Consolidated authority tiers: (score, domain_list)
    let tiers: &[(f64, &[&str])] = &[
        // High-authority academic/scholarly domains — 0.9
        (
            0.9,
            &[
                "arxiv.org",
                "scholar.google.com",
                "pubmed.ncbi.nlm.nih.gov",
                "doi.org",
                "ieeexplore.ieee.org",
                "dl.acm.org",
                "springer.com",
                "elsevier.com",
                "sciencedirect.com",
                "nature.com",
                "science.org",
                "plos.org",
                "wikipedia.org",
                "wikidata.org",
                "semanticscholar.org",
                "cell.com",
            ],
        ),
        // Medium-high authority (technical, educational, government) — 0.8
        (
            0.8,
            &[
                "github.com",
                "gitlab.com",
                "bitbucket.org",
                "docs.rs",
                "crates.io",
                "pypi.org",
                "npmjs.com",
                "stackoverflow.com",
                "stackexchange.com",
                "rust-lang.org",
                "mozilla.org",
                "chromium.org",
                "openai.com",
                "deepmind.com",
                "research.google.com",
                "mit.edu",
                "stanford.edu",
                "ox.ac.uk",
                "cam.ac.uk",
                "ibm.com",
                "microsoft.com",
                "google.com",
                "apple.com",
                "oracle.com",
                "redhat.com",
                "nginx.com",
                "docker.com",
                "cloudflare.com",
            ],
        ),
        // News outlets — 0.7
        (
            0.7,
            &[
                "reuters.com",
                "ap.org",
                "bbc.com",
                "bbc.co.uk",
                "nytimes.com",
                "wsj.com",
                "economist.com",
                "theguardian.com",
                "washingtonpost.com",
                "bloomberg.com",
                "ft.com",
                "npr.org",
                "techcrunch.com",
            ],
        ),
    ];

    for (score, domains) in tiers {
        if domains.iter().any(|h| {
            domain == *h
                || domain
                    .strip_suffix(*h)
                    .is_some_and(|rest| rest.ends_with('.'))
        }) {
            return *score;
        }
    }

    if domain.ends_with(".edu") || domain.ends_with(".gov") || domain.ends_with(".ac.uk") {
        0.8
    } else {
        0.5
    }
}

/// Compute a domain authority score (0.0-1.0) based on recognized domains.
///
/// Extracts the hostname from a URL and delegates to [`domain_authority`] for
/// the lookup, so there is a single authority source. Unknown domains default
/// to 0.5. The score helps AI agents assess source trustworthiness.
///
/// This function is public so that other crates (e.g. search, dispatch) can
/// reuse the same domain authority logic without duplicating the lookup.
pub fn compute_domain_authority(url: &str) -> f64 {
    let domain = gthings_common::url_normalizer::registered_domain(url).unwrap_or_else(|| {
        url.trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("")
            .to_lowercase()
    });
    domain_authority(&domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_authority_high() {
        assert!(domain_authority("arxiv.org") > 0.8);
        assert!(domain_authority("www.arxiv.org") > 0.8);
    }

    #[test]
    fn test_domain_authority_medium() {
        assert!(domain_authority("github.com") >= 0.5);
        assert!(domain_authority("stackoverflow.com") >= 0.5);
    }

    #[test]
    fn test_domain_authority_unknown() {
        let auth = domain_authority("example-unknown-site.com");
        assert!((0.0..=0.5).contains(&auth));
    }

    #[test]
    fn test_domain_authority_empty() {
        let auth = domain_authority("");
        assert!(auth >= 0.0);
    }
}
