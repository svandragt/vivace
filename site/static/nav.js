(function () {
  // Below 760px the sidebar is a plain, always-visible list until this runs;
  // only once it has, does the CSS collapse it behind #sidebar-toggle
  // (site/static/styles.css, ".sidebar-collapsible"). No JS means no
  // collapsing, never a dead button.
  var toggle = document.getElementById("sidebar-toggle");
  var sidebar = document.getElementById("sidebar");
  if (!toggle || !sidebar) return;
  document.body.classList.add("sidebar-collapsible");
  toggle.addEventListener("click", function () {
    var open = sidebar.classList.toggle("open");
    toggle.setAttribute("aria-expanded", String(open));
  });
})();
