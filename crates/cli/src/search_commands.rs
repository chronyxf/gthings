use common::config::GthingsConfig;
use search::types::*;

/// Format output for a single search query result.
fn output_search_results(results: &[SearchResult], meta: &SearchMeta, json: bool) {
    if json {
        let value = serde_json::json!({
            "meta": meta,
            "results": results,
        });
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else {
        println!(
            "Search results ({} total, {} ms)",
            meta.total, meta.duration_ms
        );
        println!("Query: {}", meta.query);
        println!();
        for (i, r) in results.iter().enumerate() {
            println!("{}. {}", i + 1, r.title);
            println!("   URL: {}", r.url);
            println!("   {}", r.snippet);
            println!();
        }
    }
}

/// Handler for `search query <query> [--count N]`
pub async fn handle_search_query(
    config: &GthingsConfig,
    query: &str,
    count: usize,
    json: bool,
) -> Result<(), anyhow::Error> {
    let searcher = search::GoogleSearch::new(config.clone());
    let results = searcher.query(query, count).await?;

    let meta = SearchMeta {
        total: results.len(),
        query: query.to_string(),
        duration_ms: 0,
    };

    output_search_results(&results, &meta, json);
    Ok(())
}

/// Handler for `search batch <queries...> [--count N]`
pub async fn handle_search_batch(
    config: &GthingsConfig,
    queries: &[String],
    count: usize,
    json: bool,
) -> Result<(), anyhow::Error> {
    let processor = search::BatchProcessor::new(config.clone());
    let result = processor.search(queries, count).await?;

    output_search_results(&result.results, &result.meta, json);
    Ok(())
}

/// Handler for `search harvest <queries...> [--count N] [--max N]`
pub async fn handle_search_harvest(
    config: &GthingsConfig,
    queries: &[String],
    count: usize,
    max: Option<usize>,
    json: bool,
) -> Result<(), anyhow::Error> {
    let max_pages = max.unwrap_or(count);
    let processor = search::BatchProcessor::new(config.clone());
    let result = processor.harvest(queries, count, max_pages).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    } else {
        println!(
            "Harvest complete — {} queries, {} search results, {} pages followed ({} ms)",
            result.meta.queries.len(),
            result.meta.total_search_results,
            result.meta.pages_followed,
            result.meta.duration_ms,
        );
        println!();
        if !result.search_results.is_empty() {
            println!("── Search Results ──");
            for (i, r) in result.search_results.iter().enumerate() {
                println!("{}. {} — {}", i + 1, r.title, r.url);
            }
            println!();
        }
        if !result.read_pages.is_empty() {
            println!("── Followed Pages ──");
            for (i, p) in result.read_pages.iter().enumerate() {
                let status = if p.success { "OK" } else { "FAIL" };
                let len_info = p
                    .content
                    .as_ref()
                    .map(|c| format!(" ({} chars)", c.len()))
                    .unwrap_or_default();
                println!("{}. [{}] {}{}", i + 1, status, p.url, len_info);
            }
        }
    }
    Ok(())
}
