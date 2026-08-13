/* luna site — shared behavior. No dependencies. */
(function () {
  "use strict";

  /* ---- theme toggle (persisted) ------------------------------------- */
  var root = document.documentElement;
  function setTheme(t) {
    root.setAttribute("data-theme", t);
    try { localStorage.setItem("luna-theme", t); } catch (e) {}
    var btn = document.querySelector("[data-theme-toggle]");
    if (btn) {
      btn.setAttribute("aria-label", t === "dark" ? "Switch to light theme" : "Switch to dark theme");
      btn.textContent = t === "dark" ? "☾" : "☀";
    }
  }
  document.addEventListener("click", function (e) {
    var tog = e.target.closest && e.target.closest("[data-theme-toggle]");
    if (tog) {
      var cur = root.getAttribute("data-theme") === "light" ? "light" : "dark";
      setTheme(cur === "dark" ? "light" : "dark");
    }
  });
  (function () {
    var cur = root.getAttribute("data-theme") === "light" ? "light" : "dark";
    var btn = document.querySelector("[data-theme-toggle]");
    if (btn) btn.textContent = cur === "dark" ? "☾" : "☀";
  })();

  /* ---- mobile nav --------------------------------------------------- */
  document.addEventListener("click", function (e) {
    var t = e.target.closest && e.target.closest(".nav-toggle");
    if (t) {
      var links = document.querySelector(".nav-links");
      if (links) links.classList.toggle("open");
    } else if (!(e.target.closest && e.target.closest(".nav-links"))) {
      var links2 = document.querySelector(".nav-links.open");
      if (links2) links2.classList.remove("open");
    }
  });

  /* ---- copy buttons on code blocks ---------------------------------- */
  document.addEventListener("click", function (e) {
    var btn = e.target.closest && e.target.closest(".copy");
    if (!btn) return;
    var block = btn.closest(".code");
    var pre = block && block.querySelector("pre");
    if (!pre) return;
    var text = pre.innerText;
    var done = function () {
      var old = btn.textContent;
      btn.textContent = btn.getAttribute("data-copied") || "copied";
      setTimeout(function () { btn.textContent = old; }, 1400);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, done);
    } else {
      var ta = document.createElement("textarea");
      ta.value = text; document.body.appendChild(ta); ta.select();
      try { document.execCommand("copy"); } catch (err) {}
      document.body.removeChild(ta); done();
    }
  });

  /* ---- install tabs ------------------------------------------------- */
  document.addEventListener("click", function (e) {
    var tab = e.target.closest && e.target.closest(".tab[data-tab]");
    if (!tab) return;
    var group = tab.closest("[data-tabs]");
    if (!group) return;
    var id = tab.getAttribute("data-tab");
    group.querySelectorAll(".tab[data-tab]").forEach(function (t) {
      t.setAttribute("aria-selected", t === tab ? "true" : "false");
    });
    group.querySelectorAll(".tabpanel").forEach(function (p) {
      p.hidden = p.getAttribute("data-panel") !== id;
    });
  });

  /* ---- docs sidebar scrollspy ---------------------------------------
     Position-based: pick the LAST section whose top has passed the
     reading line. An IntersectionObserver "topmost visible" pick is
     wrong here — the previous section's top goes negative while it is
     still partly on screen, so it always wins and the marker lags one
     entry behind.                                                       */
  var links = Array.prototype.slice.call(document.querySelectorAll(".sidebar a[href^='#']"));
  if (links.length) {
    var targets = links
      .map(function (a) {
        var el = document.getElementById(a.getAttribute("href").slice(1));
        return el ? { a: a, el: el } : null;
      })
      .filter(Boolean);

    if (targets.length) {
      var READ_LINE = 96;      // sticky header height + breathing room
      var SCROLL_MARGIN = 82;  // must match .doc-main > section scroll-margin-top
      var current = null;
      var pending = null;      // section a click is currently scrolling toward
      var pendingAt = 0;

      function absTop(el) {
        return el.getBoundingClientRect().top + window.pageYOffset;
      }
      function expectedY(el) {
        var max = document.documentElement.scrollHeight - window.innerHeight;
        return Math.min(Math.max(absTop(el) - SCROLL_MARGIN, 0), Math.max(max, 0));
      }
      function setActive(a) {
        if (a === current) return;
        links.forEach(function (l) { l.classList.remove("active"); });
        a.classList.add("active");
        current = a;
      }
      function sync() {
        // While a click-driven smooth scroll is in flight, hold the clicked
        // entry. A time-based lock is not enough — a long scroll outlives any
        // fixed timeout and the marker would flicker through every section on
        // the way. Hold until we actually arrive (or the user takes over).
        if (pending) {
          var arrived = Math.abs(window.pageYOffset - expectedY(pending.el)) < 8;
          if (!arrived && Date.now() - pendingAt < 4000) { setActive(pending.a); return; }
          pending = null;
        }
        var y = window.pageYOffset + READ_LINE;
        var pick = targets[0];
        for (var i = 0; i < targets.length; i++) {
          if (absTop(targets[i].el) <= y) pick = targets[i];
          else break;
        }
        // at the very bottom the last section is the one being read
        if (window.innerHeight + window.pageYOffset >= document.documentElement.scrollHeight - 2) {
          pick = targets[targets.length - 1];
        }
        setActive(pick.a);
      }

      var ticking = false;
      function onScroll() {
        if (ticking) return;
        ticking = true;
        requestAnimationFrame(function () { sync(); ticking = false; });
      }
      window.addEventListener("scroll", onScroll, { passive: true });
      window.addEventListener("resize", onScroll, { passive: true });

      targets.forEach(function (t) {
        t.a.addEventListener("click", function () {
          setActive(t.a);
          pending = t;
          pendingAt = Date.now();
        });
      });
      // any user-initiated scroll cancels the hold and hands back to the spy
      ["wheel", "touchstart", "keydown"].forEach(function (ev) {
        window.addEventListener(ev, function () { pending = null; }, { passive: true });
      });

      sync();
      window.addEventListener("load", sync);
    }
  }
})();
