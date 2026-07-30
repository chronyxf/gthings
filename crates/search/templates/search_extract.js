const _st = Date.now();
const count = __COUNT__;
const links = Array.from(document.querySelectorAll('a[href]'));
const results = [];
for (const a of links) {
  try {
    const url = a.href;
    let hostname;
    try { hostname = new URL(url).hostname; } catch(_) { continue; }
    if (hostname === location.hostname) continue;
    const title = a.textContent.trim();
    if (!title || title.length < 2) continue;
    const parent = a.closest('div.g, div[data-hveid], div[data-sokoban-container]');
    const snippetEl = parent?.querySelector('.VwiC3b, [data-sncf], span.aCOpRe, .lEBKkf, span[style*="webkit-line-clamp"]');
    const snippet = (snippetEl?.textContent || '').trim();
    results.push({ title, url, snippet, position: results.length + 1 });
    if (results.length >= count * 2) break;
  } catch(e) { continue; }
}
JSON.stringify(results);
