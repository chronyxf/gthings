const count = __COUNT__;
// Bing wraps every organic result href in a same-host redirect
// (https://www.bing.com/ck/a?...&u=<encoded>) whose `u` param carries the
// real target URL. Observed live format: a literal `a1` marker followed by
// the base64url-encoded URL (e.g. `a1aHR0cHM6Ly8...` → `https://...`), so
// the marker must be stripped BEFORE decoding — decoding the marker as part
// of the base64 stream shifts the bits and yields garbage. Unwrap BEFORE the
// same-host filter so genuine external results survive and only true
// Bing-internal links are dropped.
function unwrapRedirect(href) {
  let parsed;
  try { parsed = new URL(href); } catch (_) { return href; }
  if (parsed.pathname !== '/ck/a') return href;
  const u = parsed.searchParams.get('u');
  if (u) {
    // Try the marker-stripped form first, then the raw value, so both the
    // live `a1`+base64url format and a plain base64 `u` decode correctly.
    for (const candidate of [u.startsWith('a1') ? u.slice(2) : u, u]) {
      let b64 = candidate.replace(/-/g, '+').replace(/_/g, '/');
      while (b64.length % 4 !== 0) b64 += '=';
      try {
        const decoded = atob(b64);
        if (/^https?:\/\//i.test(decoded)) return decoded;
      } catch (_) { /* malformed base64 → try next candidate */ }
    }
  }
  return null; // /ck/a without a decodable u param is a Bing internal link
}
const blocks = Array.from(document.querySelectorAll('li.b_algo'));
const results = [];
for (const b of blocks) {
  try {
    const a = b.querySelector('h2 a[href]');
    if (!a) continue;
    const url = unwrapRedirect(a.href);
    if (!url) continue;
    let hostname;
    try { hostname = new URL(url).hostname; } catch(_) { continue; }
    if (hostname === location.hostname) continue;
    // Bing organic results: each `li.b_algo` block carries the title in an
    // `<h2><a href>` (usually the first link) and the snippet in a `<p>`.
    const title = (a.textContent || '').trim().replace(/\s+/g, ' ');
    if (!title || title.length < 2) continue;
    const snippetEl = b.querySelector('p');
    const snippet = (snippetEl?.textContent || '').trim().replace(/\s+/g, ' ');
    if (!snippet) continue;
    results.push({ title, url, snippet, position: results.length + 1 });
    if (results.length >= count * 2) break;
  } catch(e) { continue; }
}
JSON.stringify(results);
