// Tema geçişi ve aktif gezinme bağlantısı (ADR-088).
// CSP script-src 'self' satır içi script'e izin vermiyor, bu yüzden ayrı dosya.
// Metin üretmez: tema ikonu CSS sınıfı (.ico-sun/.ico-moon), aria-label şablondan
// i18n ile gelir — JS'te çevrilecek dize kalmaz (ADR-089).
(function () {
  var KEY = "opensicil-theme";
  var root = document.documentElement;

  function stored() {
    try {
      return localStorage.getItem(KEY);
    } catch (e) {
      return null; // gizli pencere / site verisi kapalı: sistem tercihi kalır
    }
  }

  function apply(theme) {
    if (theme === "dark" || theme === "light") {
      root.setAttribute("data-theme", theme);
    } else {
      root.removeAttribute("data-theme");
    }
  }

  function dark() {
    var t = root.getAttribute("data-theme");
    if (t) {
      return t === "dark";
    }
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  }

  function themeToggle() {
    var button = document.getElementById("theme-toggle");
    if (!button) {
      return;
    }
    function icon() {
      button.classList.toggle("ico-sun", dark());
      button.classList.toggle("ico-moon", !dark());
    }
    icon();
    button.addEventListener("click", function () {
      var next = dark() ? "light" : "dark";
      apply(next);
      try {
        localStorage.setItem(KEY, next);
      } catch (e) {
        // saklanamadı: seçim yalnızca bu sayfa için geçerli
      }
      icon();
    });
  }

  // Yolu en uzun eşleşen gezinme bağlantısı aktif olur; kimlik sayfaları "/" altında.
  function activeNav() {
    var path = window.location.pathname;
    var best = null;
    var bestLength = -1;
    var links = document.querySelectorAll(".side .nav-link");
    for (var i = 0; i < links.length; i++) {
      var href = links[i].getAttribute("href");
      var hit =
        path === href ||
        (href !== "/" && path.indexOf(href + "/") === 0) ||
        (href === "/" && path.indexOf("/identities") === 0);
      if (hit && href.length > bestLength) {
        best = links[i];
        bestLength = href.length;
      }
    }
    if (best) {
      best.classList.add("nav-link-active");
      best.setAttribute("aria-current", "page");
    }
  }

  apply(stored());

  document.addEventListener("DOMContentLoaded", function () {
    themeToggle();
    activeNav();
  });
})();
