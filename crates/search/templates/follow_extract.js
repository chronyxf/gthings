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
      _cl.querySelectorAll('script,style,noscript,svg,iframe,nav,footer,header').forEach(function(e) { e.remove(); });
      var _tTitle = document.title || '';
      if (_tTitle) {
        _cl.querySelectorAll('h1,h2').forEach(function(e) {
          if ((e.textContent || '').trim().toLowerCase() === _tTitle.trim().toLowerCase()) e.remove();
        });
      }
      var _text = _extractText(_cl);
      if (_text.length < 80) {
        _text = (_cl.textContent || '').replace(/[ \t]+/g, ' ').replace(/\n{2,}/g, '\n').trim();
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

  // Walk the DOM and insert a newline after every block-level element so that
  // paragraph boundaries survive even when the page is JS-rendered (where
  // innerText collapses everything into a single text node with no newlines).
  function _extractText(root) {
    var _out = [];
    var _walk = function(node) {
      if (node.nodeType === 3) { // text node
        _out.push(node.nodeValue || '');
        return;
      }
      if (node.nodeType !== 1) return; // skip non-element nodes
      var _tag = (node.tagName || '').toLowerCase();
      var _block = _isBlock(_tag);
      var _children = node.childNodes;
      for (var i = 0; i < _children.length; i++) {
        _walk(_children[i]);
      }
      if (_block) _out.push('\n');
    };
    _walk(root);
    // Normalize whitespace: collapse runs of spaces/tabs within a line to a
    // single space, and collapse 2+ newlines to a single paragraph break.
    // Newlines are preserved (never collapsed to a single line).
    return _out.join('').replace(/[ \t]+/g, ' ').replace(/\n{2,}/g, '\n').trim();
  }

  function _isBlock(tag) {
    return tag === 'p' || tag === 'div' || tag === 'h1' || tag === 'h2' || tag === 'h3' ||
      tag === 'h4' || tag === 'h5' || tag === 'h6' || tag === 'li' || tag === 'br' ||
      tag === 'section' || tag === 'article' || tag === 'blockquote' || tag === 'pre' ||
      tag === 'td' || tag === 'tr' || tag === 'ul' || tag === 'ol' || tag === 'table';
  }
})();
