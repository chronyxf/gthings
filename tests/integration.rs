// Integration tests for gthings library crates.

mod common;

use ::gthings_common::cache::Sha256DiskCache;
use gthings_extraction::html::HtmlExtractor;
use gthings_extraction::quality::{ContentQuality, QualityReason, QualityResult};

// Cache tests

#[test]
fn test_cache_key_generation() {
    let cache = Sha256DiskCache::new("/tmp/gthings-test-cache", 3600);
    let key = cache.key("https://example.com", 0, 15000);
    assert!(!key.is_empty(), "Cache key should not be empty");
    let key2 = cache.key("https://example.com", 0, 15000);
    assert_eq!(key, key2, "Same inputs should produce same key");
    let key3 = cache.key("https://example.com", 100, 15000);
    assert_ne!(key, key3, "Different offset should produce different key");
}

#[tokio::test]
async fn test_cache_set_and_get() {
    let tmp = std::env::temp_dir().join("gthings-int-cache-test");
    let _ = std::fs::remove_dir_all(&tmp);
    let cache = Sha256DiskCache::new(&tmp, 3600);
    let key = cache.key("https://test.cache/test", 0, 100);

    let miss = cache.get(&key).await.unwrap();
    assert!(miss.is_none(), "New cache key should miss");

    cache.set(&key, "test data").await;

    let hit = cache.get(&key).await.unwrap();
    assert_eq!(
        hit,
        Some("test data".to_string()),
        "Should retrieve cached data"
    );
}

#[tokio::test]
async fn test_cache_ttl_expiry() {
    let tmp = std::env::temp_dir().join("gthings-int-cache-ttl");
    let _ = std::fs::remove_dir_all(&tmp);
    // TTL=0 means immediate expiry
    let cache = Sha256DiskCache::new(&tmp, 0);
    let key = cache.key("https://test.expiry/test", 0, 100);

    cache.set(&key, "expirable data").await;
    let hit = cache.get(&key).await.unwrap();
    assert!(
        hit.is_none(),
        "TTL=0 should expire immediately. Got: {:?}",
        hit
    );
}

#[tokio::test]
async fn test_cache_persists_across_instances() {
    let tmp = std::env::temp_dir().join("gthings-int-cache-persist");
    let _ = std::fs::remove_dir_all(&tmp);

    let cache1 = Sha256DiskCache::new(&tmp, 3600);
    let key = cache1.key("https://persist.test/data", 0, 50);
    cache1.set(&key, "persistent data").await;

    let cache2 = Sha256DiskCache::new(&tmp, 3600);
    let hit = cache2.get(&key).await.unwrap();
    assert_eq!(
        hit,
        Some("persistent data".to_string()),
        "Cache should persist across instances"
    );
}

// HTML extraction tests

#[test]
fn test_html_extract_sections() {
    let html = r#"
        <html><body><article>
            <h1>Financial Report 2026</h1>
            <p>Market overview content here.</p>
            <h2>Interest Rates</h2>
            <p>The Fed held rates at 3.50-3.75%.</p>
            <h2>Inflation Outlook</h2>
            <p>PCE inflation at 4.1%.</p>
        </article></body></html>
    "#;

    let result = HtmlExtractor::extract(html, "article").unwrap();
    assert!(!result.sections.is_empty(), "Should detect sections");
    assert!(
        result
            .sections
            .iter()
            .any(|s| s.heading == "Financial Report 2026"),
        "Should find h1 heading"
    );
    assert!(
        result
            .sections
            .iter()
            .any(|s| s.heading == "Interest Rates"),
        "Should find h2 heading"
    );
}

#[test]
fn test_html_extract_empty() {
    let result = HtmlExtractor::extract("", "body").unwrap();
    assert!(
        result.content.is_empty(),
        "Empty HTML should give empty content"
    );
    assert_eq!(result.total_length, 0);
    assert!(result.sections.is_empty());
}

#[test]
fn test_html_extract_fallback_selector() {
    let html = r#"<html><body><p>Fallback content works.</p></body></html>"#;
    let result = HtmlExtractor::extract(html, "nonexistent-selector").unwrap();
    assert!(
        result.content.contains("Fallback content"),
        "Should fallback to body"
    );
}

#[test]
fn test_strip_tags_simple() {
    let result = HtmlExtractor::strip_tags("<p>Hello <b>world</b></p>");
    assert_eq!(result, "Hello world");
}

#[test]
fn test_strip_tags_with_entities() {
    let result = HtmlExtractor::strip_tags("<p>AT&amp;T &lt; test</p>");
    assert_eq!(result, "AT&T < test");
}

#[test]
fn test_strip_tags_empty() {
    assert_eq!(HtmlExtractor::strip_tags(""), "");
    assert_eq!(HtmlExtractor::strip_tags("<div></div>"), "");
}

#[test]
fn test_detect_sections_from_text() {
    // All-caps heading detection (needs 15+ chars)
    let text = "INTRODUCTION TO THE STUDY\nSome intro text here.\n\nMAIN METHODOLOGY USED\nMethod details here.\n";
    let sections = HtmlExtractor::detect_sections(text);
    assert!(!sections.is_empty(), "Should detect ALL CAPS sections");
    assert!(
        sections
            .iter()
            .any(|s| s.heading.contains("INTRODUCTION TO THE STUDY"))
    );
}

#[test]
fn test_detect_sections_colon_heading() {
    let text = "Introduction:\nIntro content.\n\nResults:\nResult content.\n";
    let sections = HtmlExtractor::detect_sections(text);
    assert!(!sections.is_empty(), "Should detect colon headings");
}

// Quality validation tests

#[test]
fn test_quality_valid_content() {
    let text = "This is a sufficiently long piece of text with natural language. \
                It has sentences, punctuation, and enough words to pass. \
                This content should be considered acceptable.";
    let result = ContentQuality::validate(text);
    assert!(result.is_ok, "Valid text should pass quality gate");
    assert!(result.score >= 0.5, "Score should be >= 0.5");
}

#[test]
fn test_quality_empty_content() {
    let result = ContentQuality::validate("");
    assert!(!result.is_ok, "Empty should fail");
    assert_eq!(result.score, 0.0);
    assert!(result.reasons.contains(&QualityReason::EmptyContent));
}

#[test]
fn test_quality_too_short() {
    let result = ContentQuality::validate("Hi");
    assert!(!result.is_ok);
    assert!(result.reasons.contains(&QualityReason::TooShort));
}

#[test]
fn test_quality_bot_detection() {
    assert!(ContentQuality::detect_bot(
        "Checking your browser before accessing"
    ));
    assert!(ContentQuality::detect_bot("Just a moment..."));
    assert!(ContentQuality::detect_bot("cloudflare challenge"));
    assert!(!ContentQuality::detect_bot("Normal article content here"));
}

#[test]
fn test_quality_captcha_detection() {
    assert!(ContentQuality::detect_captcha("recaptcha widget"));
    assert!(ContentQuality::detect_captcha("cf-turnstile"));
    assert!(!ContentQuality::detect_captcha("normal content"));
}

#[test]
fn test_quality_paywall_detection() {
    assert!(ContentQuality::detect_paywall(
        "Subscribe now to continue reading"
    ));
    assert!(ContentQuality::detect_paywall(
        "Log in to read this article"
    ));
    assert!(!ContentQuality::detect_paywall(
        "This is normal article content"
    ));
}

#[test]
fn test_quality_empty_shell() {
    assert!(ContentQuality::detect_empty_shell("short"));
    assert!(ContentQuality::detect_empty_shell(
        "Please enable JavaScript to view this page."
    ));
    assert!(!ContentQuality::detect_empty_shell(
        "This is a sufficiently long text with many words that should not be detected as an empty shell."
    ));
}

#[test]
fn test_quality_needs_recrawl() {
    let low = QualityResult {
        score: 0.2,
        is_ok: false,
        reasons: vec![QualityReason::TooShort],
        length: 10,
    };
    assert!(ContentQuality::needs_recrawl(&low));

    let good = QualityResult {
        score: 0.7,
        is_ok: true,
        reasons: vec![],
        length: 1000,
    };
    assert!(!ContentQuality::needs_recrawl(&good));
}

#[test]
fn test_quality_secondary_check() {
    let result = ContentQuality::secondary_check("Just a few words");
    assert!(result.sparse, "Few words should be sparse");

    let repetitive = "This is a repeated sentence. This is a repeated sentence. This is a repeated sentence. This is a repeated sentence.";
    let r = ContentQuality::secondary_check(repetitive);
    assert!(r.repetitive, "Repeated content should be detected");
}

// Search type tests

#[test]
fn test_follow_result_serialization() {
    let result = gthings_search::types::FollowResult {
        url: "https://example.com".into(),
        content: Some("test content".into()),
        total_length: 12,
        offset: 0,
        sections: vec![gthings_extraction::html::Section {
            heading: "Title".into(),
            content: "Body text".into(),
        }],
        error: None,
        quality: Some(gthings_extraction::quality::QualityResult {
            score: 1.0,
            is_ok: true,
            reasons: vec![],
            length: 12,
        }),
        success: true,
        truncated: false,
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(
        json.contains("\"sections\""),
        "JSON should include sections"
    );
    assert!(json.contains("\"quality\""), "JSON should include quality");
    assert!(
        json.contains("\"heading\":\"Title\""),
        "JSON should include heading"
    );

    // Round-trip
    let parsed: gthings_search::types::FollowResult = serde_json::from_str(&json).unwrap();
    assert!(parsed.success);
    assert_eq!(parsed.url, "https://example.com");
    assert_eq!(parsed.sections.len(), 1);
}

#[test]
fn test_harvest_meta_serialization() {
    let meta = gthings_search::types::HarvestMeta {
        queries: vec!["fed rates".into(), "inflation".into()],
        total_search_results: 10,
        unique_urls: 7,
        pages_followed: 5,
        pages_skipped: 2,
        duration_ms: 3500,
    };

    let json = serde_json::to_string(&meta).unwrap();
    assert!(
        json.contains("\"unique_urls\":7"),
        "Should include unique_urls"
    );
    assert!(
        json.contains("\"pages_skipped\":2"),
        "Should include pages_skipped"
    );
    assert!(
        json.contains("\"duration_ms\":3500"),
        "Should include duration_ms"
    );

    let parsed: gthings_search::types::HarvestMeta = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.unique_urls, 7);
    assert_eq!(parsed.pages_skipped, 2);
}

#[test]
fn test_search_result_with_query() {
    let result = gthings_search::types::SearchResult {
        title: "Fed Rate 2026".into(),
        url: "https://example.com/fed".into(),
        snippet: "The Fed kept rates at 3.50-3.75%".into(),
        query: Some("fed rates 2026".into()),
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"query\""), "Should include query field");
}

#[test]
fn test_follow_opts_defaults() {
    let opts = gthings_search::types::FollowOpts::default();
    assert_eq!(opts.selector, "article,main,[role=main]");
    assert_eq!(opts.offset, 0);
    assert_eq!(opts.max_length, 15000);
    assert_eq!(opts.timeout_ms, 30000);
    assert!(opts.retry_on_low_quality);
}

#[cfg(test)]
mod skill_commands {
    use std::path::PathBuf;
    use std::process::Command;

    fn gthings_binary() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("target");
        path.push("debug");
        path.push("gthings");
        if !path.exists() {
            path.set_extension("exe");
        }
        path
    }

    fn temp_home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let home = dir.path().to_path_buf();
        (dir, home)
    }

    #[test]
    fn test_skill_add_opencode_installs_files() {
        let (_dir, home) = temp_home();
        let output = Command::new(gthings_binary())
            .env("HOME", &home)
            .args(["skill", "add", "--opencode"])
            .output()
            .expect("run gthings skill add --opencode");

        assert!(
            output.status.success(),
            "skill add --opencode should succeed"
        );

        let skill_md = home
            .join(".config")
            .join("opencode")
            .join("skills")
            .join("gthings")
            .join("SKILL.md");
        assert!(
            skill_md.exists(),
            "SKILL.md should be installed to opencode: {:?}",
            skill_md
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Skill installation complete"),
            "should report completion"
        );
    }

    #[test]
    fn test_skill_add_agents_installs_files() {
        let (_dir, home) = temp_home();
        let output = Command::new(gthings_binary())
            .env("HOME", &home)
            .args(["skill", "add", "--agents"])
            .output()
            .expect("run gthings skill add --agents");

        assert!(output.status.success(), "skill add --agents should succeed");

        let skill_md = home
            .join(".agents")
            .join("skills")
            .join("gthings")
            .join("SKILL.md");
        assert!(
            skill_md.exists(),
            "SKILL.md should be installed to agents: {:?}",
            skill_md
        );

        let reference_dir = home
            .join(".agents")
            .join("skills")
            .join("gthings")
            .join("reference");
        assert!(
            reference_dir.join("commands.md").exists(),
            "commands.md should be installed"
        );
        assert!(
            reference_dir.join("quality.md").exists(),
            "quality.md should be installed"
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Skill installation complete"),
            "should report completion"
        );
    }

    #[test]
    fn test_skill_add_all_installs_both() {
        let (_dir, home) = temp_home();
        let output = Command::new(gthings_binary())
            .env("HOME", &home)
            .args(["skill", "add", "--all"])
            .output()
            .expect("run gthings skill add --all");

        assert!(output.status.success(), "skill add --all should succeed");

        let opencode_skill = home
            .join(".config")
            .join("opencode")
            .join("skills")
            .join("gthings")
            .join("SKILL.md");
        assert!(opencode_skill.exists(), "opencode SKILL.md should exist");

        let agents_skill = home
            .join(".agents")
            .join("skills")
            .join("gthings")
            .join("SKILL.md");
        assert!(agents_skill.exists(), "agents SKILL.md should exist");
    }

    #[test]
    fn test_skill_add_no_flag_fails() {
        let (_dir, home) = temp_home();
        let output = Command::new(gthings_binary())
            .env("HOME", &home)
            .args(["skill", "add"])
            .output()
            .expect("run gthings skill add (no flags)");

        assert!(
            !output.status.success(),
            "skill add without flags should fail"
        );
    }

    #[test]
    fn test_update_shows_help() {
        let output = Command::new(gthings_binary())
            .args(["--help"])
            .output()
            .expect("run gthings --help");

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("update"), "help should show update command");
        assert!(stdout.contains("skill"), "help should show skill command");
    }
}
