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

  // Tablo başlığındaki kutu, aynı formdaki data-select-all="<ad>" ile adı verilen
  // bütün kutuları seçer/bırakır; satırlar değişince başlık kutusu onları yansıtır
  // (hepsi seçiliyse işaretli, bir kısmıysa belirsiz). ADR-103 madde 3.
  function selectAll() {
    var heads = document.querySelectorAll("input[type=checkbox][data-select-all]");
    for (var i = 0; i < heads.length; i++) {
      bindSelectAll(heads[i]);
    }
  }

  function bindSelectAll(head) {
    var form = head.form || head.closest("form");
    if (!form) {
      return;
    }
    var name = head.getAttribute("data-select-all");
    function rows() {
      return form.querySelectorAll('input[type=checkbox][name="' + name + '"]');
    }
    function reflect() {
      var list = rows();
      var checked = 0;
      for (var i = 0; i < list.length; i++) {
        if (list[i].checked) {
          checked++;
        }
      }
      head.checked = list.length > 0 && checked === list.length;
      head.indeterminate = checked > 0 && checked < list.length;
    }
    head.addEventListener("change", function () {
      var list = rows();
      for (var i = 0; i < list.length; i++) {
        list[i].checked = head.checked;
      }
      head.indeterminate = false;
    });
    form.addEventListener("change", function (event) {
      if (event.target !== head && event.target.name === name) {
        reflect();
      }
    });
    reflect();
  }

  apply(stored());

  document.addEventListener("DOMContentLoaded", function () {
    themeToggle();
    activeNav();
    selectAll();
  });
})();
