(async function() {
  try {
    var _deadline = Date.now() + 3000;
    while (Date.now() < _deadline) {
      if (document.body && document.body.innerText && document.body.innerText.length > 100) break;
      await new Promise(function(r) { setTimeout(r, 100); });
    }
    var _c = document.querySelector('main, article, [role="main"]') ?? document.body;
    if (!_c) {
      return JSON.stringify({ title: document.title || '', content: '', error: 'No document body found' });
    } else {
      var _isMain = _c !== document.body;
      var _cl = _c.cloneNode(true);
      if (_isMain) {
        _cl.querySelectorAll('script,style,noscript,svg,iframe,nav,footer,header').forEach(function(e) { e.remove(); });
      } else {
        _cl.querySelectorAll('script,style,noscript,svg,iframe').forEach(function(e) { e.remove(); });
      }
      var _text = _cl.innerText || '';
      if (_text.length < 80) {
        _text = _cl.textContent || '';
        _text = _text.replace(/\s+/g, ' ').trim();
      }
      var _title = document.title || '';
      if (_text.length < 3) {
        return JSON.stringify({ title: _title, content: '', error: 'content too short (' + _text.length + ' chars)' });
      } else {
        var _t = _text.substring(__OFFSET__, __MAX_CHARS__);
        return JSON.stringify({ title: _title, content: _t, error: '' });
      }
    }
  } catch (e) {
    return JSON.stringify({ title: document.title || '', content: '', error: e.message });
  }
})();
