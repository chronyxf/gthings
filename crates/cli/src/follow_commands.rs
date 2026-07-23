use common::config::GthingsConfig;
use common::trace::TraceWriter;
use search::types::FollowOpts;

/// Format and print a single [`search::types::FollowResult`].
fn output_follow_result(
    result: &search::types::FollowResult,
    json: bool,
) -> Result<(), anyhow::Error> {
    if json {
        let output = serde_json::to_string_pretty(result)?;
        println!("{}", output);
    } else {
        let status = if result.success { "OK" } else { "FAIL" };
        println!("[{}] {}", status, result.url);
        if let Some(ref content) = result.content {
            println!(
                "  Length: {} chars (total page: {}, truncated: {})",
                content.len(),
                result.total_length,
                result.truncated,
            );
            // Print first 3 lines as preview
            for line in content.lines().take(3) {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    println!("  │ {}", trimmed);
                }
            }
        }
        if let Some(ref error) = result.error {
            println!("  Error: {}", error);
        }
        if let Some(ref quality) = result.quality {
            println!("  Quality: score={}, ok={}", quality.score, quality.is_ok);
        }
        println!();
    }
    Ok(())
}

/// Handler for `follow url <url> [--selector S] [--offset N] [--max N]`
pub(crate) async fn handle_follow_url(
    config: &GthingsConfig,
    url: &str,
    selector: &str,
    offset: usize,
    max: usize,
    json: bool,
    trace: Option<&mut TraceWriter>,
) -> Result<(), anyhow::Error> {
    let follower = search::PageFollower::new(config.clone());
    let opts = FollowOpts {
        selector: selector.to_string(),
        offset,
        max_length: max,
        ..FollowOpts::default()
    };
    let result = follower.follow(url, opts, trace).await?;
    output_follow_result(&result, json)?;
    Ok(())
}

/// Handler for `follow batch <urls...> [--selector S] [--offset N] [--max N]`
pub(crate) async fn handle_follow_batch(
    config: &GthingsConfig,
    urls: &[String],
    selector: &str,
    offset: usize,
    max: usize,
    json: bool,
    trace: Option<&mut TraceWriter>,
) -> Result<(), anyhow::Error> {
    let follower = search::PageFollower::new(config.clone());
    let opts = FollowOpts {
        selector: selector.to_string(),
        offset,
        max_length: max,
        ..FollowOpts::default()
    };
    let results = follower.batch(urls, opts, trace).await?;

    if json {
        let output = serde_json::to_string_pretty(&results)?;
        println!("{}", output);
    } else {
        for result in &results {
            output_follow_result(result, json)?;
        }
    }
    Ok(())
}
