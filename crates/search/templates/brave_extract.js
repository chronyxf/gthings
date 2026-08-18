const count = __COUNT__;
const blocks = Array.from(document.querySelectorAll('div[data-type="web"]'));
const results = [];
for (const b of blocks) {
  try {
    const a = b.querySelector('a[href]');
    if (!a) continue;
    const url = a.href;
    let hostname;
    try { hostname = new URL(url).hostname; } catch(_) { continue; }
    if (hostname === location.hostname) continue;
    // Stable semantic markers only — the scoped svelte class hashes are
    // per-build and unstable, so the title/snippet/url selectors rely on
    // the semantic classes (search-snippet-title / generic-snippet) that
    // the live SERP emits inside each data-type="web" block.
    const titleEl = b.querySelector('.title.search-snippet-title');
    const title = (titleEl?.textContent || '').trim().replace(/\s+/g, ' ');
    if (!title || title.length < 2) continue;
    const snippetEl = b.querySelector('.generic-snippet .content');
    const snippet = (snippetEl?.textContent || '').trim().replace(/\s+/g, ' ');
    if (!snippet) continue;
    results.push({ title, url, snippet, position: results.length + 1 });
    if (results.length >= count * 2) break;
  } catch(e) { continue; }
}
JSON.stringify(results);
