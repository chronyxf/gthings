const _st = Date.now();
const count = __COUNT__;
const links = Array.from(document.querySelectorAll('a[href]'));
const results = [];
// Google's own nav/utility paths (images, maps, news, account, ...) that
// must never be treated as organic results.
const NAV_RE = /^\/(search|url|maps|images|news|videos|shopping|books|finance|account|preferences|intl|services|about|support|policies|howsearchworks|advanced_search|settings|webhp|complete|sorry|local|travel|flights|hotels|translate|docs|drive|gmail|calendar|contacts|photos|meet|tasks|forms|sites|myaccount|signin|logout)/;
function isNavOrUtility(url) {
  try {
    const u = new URL(url);
    const h = u.hostname.replace(/^www\./, '');
    if (h === 'google.com') return NAV_RE.test(u.pathname);
    return false;
  } catch(_) { return true; }
}
for (const a of links) {
  try {
    const url = a.href;
    let hostname;
    try { hostname = new URL(url).hostname; } catch(_) { continue; }
    if (hostname === location.hostname) continue;
    if (isNavOrUtility(url)) continue;
    const title = a.textContent.trim();
    if (!title || title.length < 2) continue;
    // Organic-result container only (stable data-attribute selectors; the
    // legacy class-based 'div.g' is dropped as brittle).
    const parent = a.closest('div[data-hveid], div[data-sokoban-container], div[data-async-context]');
    // Snippet must match before the result is accepted.
    const snippetEl = parent?.querySelector('[data-sncf], span[style*="webkit-line-clamp"], .VwiC3b');
    const snippet = (snippetEl?.textContent || '').trim().replace(/\s+/g, ' ');
    if (!snippet) continue;
    results.push({ title, url, snippet, position: results.length + 1 });
    if (results.length >= count * 2) break;
  } catch(e) { continue; }
}
JSON.stringify(results);
