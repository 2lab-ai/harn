/* harn landing: theme (runs in <head>, before first paint), tabs, copy. No dependencies.
   Without JavaScript the page is complete: every frame is shown, the copy and theme buttons stay hidden. */
(function () {
  "use strict";
  var root = document.documentElement;
  var KEY = "harn-theme";
  var dark = window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : null;

  root.classList.add("js");

  function stored() {
    try { return localStorage.getItem(KEY); } catch (e) { return null; }
  }
  function store(v) {
    try { if (v) localStorage.setItem(KEY, v); else localStorage.removeItem(KEY); } catch (e) { /* private mode */ }
  }
  function applyTheme(v) {
    if (v === "light" || v === "dark") root.setAttribute("data-theme", v);
    else root.removeAttribute("data-theme");
  }
  function systemDark() { return !!(dark && dark.matches); }
  function effectiveDark() {
    var t = root.getAttribute("data-theme");
    return t ? t === "dark" : systemDark();
  }

  applyTheme(stored());

  function initTheme() {
    var btn = document.querySelector(".theme");
    if (!btn) return;
    function sync() { btn.setAttribute("aria-pressed", effectiveDark() ? "true" : "false"); }
    btn.addEventListener("click", function () {
      var next = !effectiveDark();
      // Back to the system default when the choice matches it; remembered only when it differs.
      var v = next === systemDark() ? null : (next ? "dark" : "light");
      applyTheme(v);
      store(v);
      sync();
    });
    if (dark && dark.addEventListener) dark.addEventListener("change", sync);
    btn.hidden = false;
    sync();
  }

  function initTabs() {
    var list = document.querySelector("[role=tablist]");
    if (!list) return;
    var tabs = Array.prototype.slice.call(list.querySelectorAll("[role=tab]"));
    var panels = tabs.map(function (t) { return document.getElementById(t.getAttribute("aria-controls")); });
    panels.forEach(function (p, i) {
      p.setAttribute("role", "tabpanel");
      p.setAttribute("aria-labelledby", tabs[i].id);
    });
    function select(i, focus) {
      tabs.forEach(function (t, j) {
        var on = i === j;
        t.setAttribute("aria-selected", on ? "true" : "false");
        t.tabIndex = on ? 0 : -1;
        panels[j].classList.toggle("is-on", on);
        panels[j].hidden = !on;
      });
      if (focus) tabs[i].focus();
    }
    list.addEventListener("click", function (e) {
      var i = tabs.indexOf(e.target.closest("[role=tab]"));
      if (i >= 0) select(i, false);
    });
    list.addEventListener("keydown", function (e) {
      var i = tabs.indexOf(document.activeElement);
      if (i < 0) return;
      var n = tabs.length, to = -1;
      if (e.key === "ArrowRight" || e.key === "ArrowDown") to = (i + 1) % n;
      else if (e.key === "ArrowLeft" || e.key === "ArrowUp") to = (i - 1 + n) % n;
      else if (e.key === "Home") to = 0;
      else if (e.key === "End") to = n - 1;
      if (to < 0) return;
      e.preventDefault();
      select(to, true);
    });
    list.hidden = false;
    select(0, false);
  }

  function initCopy() {
    Array.prototype.forEach.call(document.querySelectorAll("[data-copy]"), function (btn) {
      var box = btn.parentNode;
      var status = box.querySelector("[role=status]");
      var text = box.querySelector(".cmd-text");
      var timer = 0;
      function say(label, spoken) {
        btn.textContent = label;
        if (status) status.textContent = spoken;
        clearTimeout(timer);
        timer = setTimeout(function () {
          btn.textContent = "copy";
          if (status) status.textContent = "";
        }, 1500);
      }
      function selectText() {
        var sel = window.getSelection && window.getSelection();
        if (!sel || !text) return;
        var r = document.createRange();
        r.selectNodeContents(text);
        sel.removeAllRanges();
        sel.addRange(r);
        say("selected", "Install command selected; copy it with your keyboard.");
      }
      btn.addEventListener("click", function () {
        var value = btn.getAttribute("data-copy");
        if (navigator.clipboard && window.isSecureContext) {
          navigator.clipboard.writeText(value).then(function () {
            say("copied", "Install command copied.");
          }, selectText);
        } else {
          selectText();
        }
      });
      btn.hidden = false;
    });
  }

  function init() { initTheme(); initTabs(); initCopy(); }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
