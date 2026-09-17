(function () {
  var input = document.getElementById("site-search");
  var panel = document.getElementById("search-results");
  if (!input || !panel) return;

  var records = null;
  var loading = null;

  function load() {
    if (!loading) {
      loading = fetch("search.json").then(function (r) { return r.json(); }).then(function (data) {
        records = data;
      });
    }
    return loading;
  }

  // Records hold unescaped text (the build strips tags and entities), so
  // every string goes through this before it reaches innerHTML.
  function esc(s) {
    return s.replace(/[&<>"]/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
    });
  }

  function snippet(text, term) {
    var lower = text.toLowerCase();
    var pos = term ? lower.indexOf(term) : -1;
    if (pos === -1) return text.slice(0, 120);
    var start = Math.max(0, pos - 40);
    var end = Math.min(text.length, start + 120);
    return (start > 0 ? "…" : "") + text.slice(start, end) + (end < text.length ? "…" : "");
  }

  function render(query) {
    var terms = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (!terms.length) {
      panel.hidden = true;
      panel.innerHTML = "";
      return;
    }
    var matches = [];
    for (var i = 0; i < records.length; i++) {
      var rec = records[i];
      var haystack = (rec.heading + " " + rec.text).toLowerCase();
      var hits = 0;
      var ok = true;
      for (var j = 0; j < terms.length; j++) {
        if (haystack.indexOf(terms[j]) === -1) { ok = false; break; }
        hits += haystack.split(terms[j]).length - 1;
      }
      if (!ok) continue;
      var headingLower = rec.heading.toLowerCase();
      var headingHit = terms.every(function (t) { return headingLower.indexOf(t) !== -1; });
      matches.push({ rec: rec, headingHit: headingHit, hits: hits });
    }
    matches.sort(function (a, b) {
      if (a.headingHit !== b.headingHit) return a.headingHit ? -1 : 1;
      return b.hits - a.hits;
    });
    matches = matches.slice(0, 8);
    panel.hidden = false;
    if (!matches.length) {
      panel.innerHTML = "<p>No matches</p>";
      return;
    }
    panel.innerHTML = matches.map(function (m) {
      var snip = snippet(m.rec.text, terms[0]);
      return '<a href="' + esc(m.rec.url) + '"><strong>' + esc(m.rec.heading) + '</strong> <span>' +
        esc(m.rec.page) + '</span><p>' + esc(snip) + '</p></a>';
    }).join("");
  }

  input.addEventListener("input", function () {
    var query = input.value;
    load().then(function () { render(query); });
  });

  input.addEventListener("keydown", function (e) {
    if (e.key === "Escape") {
      input.value = "";
      panel.hidden = true;
      panel.innerHTML = "";
    }
  });
})();
