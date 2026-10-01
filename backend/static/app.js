// Tema geçişi (ADR-088): varsayılan sistem tercihi, elle seçim localStorage'da.
// CSP script-src 'self' satır içi script'e izin vermiyor, bu yüzden ayrı dosya.
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

  apply(stored());

  document.addEventListener("DOMContentLoaded", function () {
    var button = document.getElementById("theme-toggle");
    if (!button) {
      return;
    }
    function label() {
      button.textContent = dark() ? "☀" : "☾";
      button.setAttribute(
        "aria-label",
        dark() ? "Açık temaya geç" : "Koyu temaya geç",
      );
    }
    label();
    button.addEventListener("click", function () {
      var next = dark() ? "light" : "dark";
      apply(next);
      try {
        localStorage.setItem(KEY, next);
      } catch (e) {
        // saklanamadı: seçim yalnızca bu sayfa için geçerli
      }
      label();
    });
  });
})();
