// Optional enhancements. The site is fully usable without this file.
(function () {
  var root = document.documentElement;
  var KEY = 'mide-theme';
  function stored() { try { return localStorage.getItem(KEY); } catch (e) { return null; } }
  function current() {
    var s = stored();
    if (s === 'light' || s === 'dark') return s;
    return window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  }
  function apply(t) { root.setAttribute('data-theme', t); }
  apply(current());

  var btn = document.getElementById('theme-toggle');
  if (btn) {
    function label() { btn.textContent = current() === 'dark' ? 'Light theme' : 'Dark theme'; }
    label();
    btn.addEventListener('click', function () {
      var next = current() === 'dark' ? 'light' : 'dark';
      try { localStorage.setItem(KEY, next); } catch (e) {}
      apply(next); label();
    });
  }

  // Copy buttons on code blocks.
  document.querySelectorAll('pre').forEach(function (pre) {
    if (pre.dataset.nocopy !== undefined) return;
    var b = document.createElement('button');
    b.className = 'copy'; b.type = 'button'; b.textContent = 'Copy';
    b.addEventListener('click', function () {
      var text = pre.querySelector('code') ? pre.querySelector('code').innerText : pre.innerText;
      var done = function () { b.textContent = 'Copied'; setTimeout(function () { b.textContent = 'Copy'; }, 1400); };
      if (navigator.clipboard) { navigator.clipboard.writeText(text.replace(/\n$/, '')).then(done, function () {}); }
    });
    pre.appendChild(b);
  });
})();
