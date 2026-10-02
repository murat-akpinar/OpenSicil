// Tema geçişi ve aktif gezinme bağlantısı (ADR-088).
// CSP script-src 'self' satır içi script'e izin vermiyor, bu yüzden ayrı dosya.
// Metin üretmez: tema ikonu CSS sınıfı (.ico-sun/.ico-moon), aria-label şablondan
// i18n ile gelir — JS'te çevrilecek dize kalmaz (ADR-089).
(function () {
  var KEY = "opensicil-theme";
  var NAV_KEY = "opensicil-nav";
  // Bu genişliğin altındaki monitör dar sayılır ve menü daraltılmış açılır.
  // Operatör bir kez seçim yaparsa seçimi kazanır, genişlik artık karışmaz.
  var WIDE_MONITOR = 1280;
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

  // --- kenar çubuğu daraltma (monitör genişliğine göre varsayılan) ---
  function navStored() {
    try {
      return localStorage.getItem(NAV_KEY);
    } catch (e) {
      return null;
    }
  }

  function navDefault() {
    return window.innerWidth >= WIDE_MONITOR ? "expanded" : "collapsed";
  }

  // İlk çizimden önce çağrılır: sonra çağrılsa menü geniş açılıp daralırdı.
  function navApply(state) {
    root.setAttribute("data-nav", state === "collapsed" ? "collapsed" : "expanded");
  }

  function navCollapse() {
    var button = document.getElementById("nav-collapse");
    if (!button) {
      return;
    }
    var icon = button.querySelector(".ico");
    var label = button.querySelector(".nav-label");
    function reflect() {
      var collapsed = root.getAttribute("data-nav") === "collapsed";
      var text = button.getAttribute(collapsed ? "data-label-expand" : "data-label-collapse");
      if (icon) {
        icon.classList.toggle("ico-chevron-right", collapsed);
        icon.classList.toggle("ico-chevron-left", !collapsed);
      }
      if (label) {
        label.textContent = text;
      }
      button.setAttribute("aria-label", text);
      button.setAttribute("aria-expanded", collapsed ? "false" : "true");
    }
    reflect();
    button.addEventListener("click", function () {
      var next = root.getAttribute("data-nav") === "collapsed" ? "expanded" : "collapsed";
      navApply(next);
      try {
        localStorage.setItem(NAV_KEY, next);
      } catch (e) {
        // saklanamadı: seçim yalnızca bu sayfa için geçerli
      }
      reflect();
    });

    // Seçim yapılmadıysa monitör/pencere genişliği varsayılanı sürdürür:
    // dizüstünü harici ekrana takan operatör menüyü elle açmak zorunda kalmasın.
    if (!navStored() && window.matchMedia) {
      var wide = window.matchMedia("(min-width: " + WIDE_MONITOR + "px)");
      var onChange = function () {
        if (navStored()) {
          return;
        }
        navApply(navDefault());
        reflect();
      };
      if (wide.addEventListener) {
        wide.addEventListener("change", onChange);
      } else if (wide.addListener) {
        wide.addListener(onChange);
      }
    }
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

  // Mobil gezinme çekmecesi: düğme `.side`e ve karartmaya `data-open` yazar.
  // Satır içi `onclick` CSP'de yasak, bu yüzden bağlama burada.
  function navDrawer() {
    var button = document.getElementById("nav-toggle");
    var side = document.getElementById("side");
    var scrim = document.getElementById("nav-scrim");
    if (!button || !side) {
      return;
    }
    function open(yes) {
      if (yes) {
        side.setAttribute("data-open", "");
        if (scrim) {
          scrim.setAttribute("data-open", "");
        }
      } else {
        side.removeAttribute("data-open");
        if (scrim) {
          scrim.removeAttribute("data-open");
        }
      }
      button.setAttribute("aria-expanded", yes ? "true" : "false");
    }
    button.addEventListener("click", function () {
      open(!side.hasAttribute("data-open"));
    });
    if (scrim) {
      scrim.addEventListener("click", function () {
        open(false);
      });
    }
    document.addEventListener("keydown", function (event) {
      if (event.key === "Escape") {
        open(false);
      }
    });
  }

  // Yapışkan tablo başlığı üst barın altına oturmalı; üst barın yüksekliği
  // sabit değil (dar ekranda iki satıra sarıyor), bu yüzden ölçülüp
  // `--topbar-h`e yazılır. CSSOM'a yazmak CSP'ye takılmaz: yasak olan
  // HTML'deki `style` özniteliği. Ölçüm başarısızsa CSS'teki yedek değer kalır.
  function topbarHeight() {
    var bar = document.querySelector(".topbar");
    if (!bar) {
      return;
    }
    function set() {
      root.style.setProperty("--topbar-h", bar.offsetHeight + "px");
    }
    set();
    if (window.ResizeObserver) {
      new ResizeObserver(set).observe(bar);
    } else {
      window.addEventListener("resize", set);
    }
  }

  // Satırın tamamı tıklanabilir: satırdaki ilk bağlantı nereye gidiyorsa oraya.
  // Bağlantı, buton ve onay kutusu kendi işini yapar; metin seçmek engellenmez.
  function clickableRows() {
    var rows = document.querySelectorAll(".tbl tbody tr");
    for (var i = 0; i < rows.length; i++) {
      var link = rows[i].querySelector("a[href]");
      if (!link) {
        continue;
      }
      rows[i].setAttribute("data-href", link.getAttribute("href"));
      rows[i].addEventListener("click", rowClick);
    }
  }

  function rowClick(event) {
    if (event.target.closest("a, button, input, label, select, textarea")) {
      return;
    }
    if (String(window.getSelection())) {
      return; // metin seçiliyor
    }
    window.location.href = this.getAttribute("data-href");
  }

  // Sayı kolonundaki sıfırlar solar: dolu bir listede "0" göz için gürültü.
  function dimZeros() {
    var cells = document.querySelectorAll(".tbl td.num");
    for (var i = 0; i < cells.length; i++) {
      if (cells[i].textContent.trim() === "0") {
        cells[i].classList.add("zero");
      }
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

  // Baş harf avatarının rengi kişiye göre sabit: ad hash'lenir, sekiz grafik
  // renginden biri seçilir. Aynı kişi her listede aynı renkte görünür; renk
  // satır içi `style` ile değil `.avatar-cN` sınıfıyla verilir (CSP).
  var AVATAR_COLORS = 8;

  function avatarColors() {
    var avatars = document.querySelectorAll(".avatar");
    for (var i = 0; i < avatars.length; i++) {
      // Kabuktaki kullanıcı çipi vurgu renginde kalır: o "ben"im, listeden ayrılsın
      if (avatars[i].closest(".userchip")) {
        continue;
      }
      var seed = avatars[i].getAttribute("data-seed") || avatars[i].textContent;
      avatars[i].classList.add("avatar-c" + (hash(seed) % AVATAR_COLORS + 1));
    }
  }

  // djb2: kısa, çakışması önemsiz (renk seçiyor, kimlik üretmiyor)
  function hash(text) {
    var h = 5381;
    for (var i = 0; i < text.length; i++) {
      h = (h * 33 + text.charCodeAt(i)) & 0x7fffffff;
    }
    return h;
  }

  // `data-ratio` kutusu: aynı kolondaki en büyük sayıya göre oran çubuğu. Genişlik
  // hazır `.v-NN` sınıflarından gelir (beşer adım), satır içi `style` CSP'de yasak.
  function ratioBars() {
    var tables = document.querySelectorAll(".tbl");
    for (var t = 0; t < tables.length; t++) {
      fillRatios(tables[t]);
    }
  }

  function fillRatios(table) {
    var slots = table.querySelectorAll("[data-ratio]");
    if (!slots.length) {
      return;
    }
    var values = [];
    var max = 0;
    for (var i = 0; i < slots.length; i++) {
      var cell = slots[i].closest("td");
      var n = parseInt((cell ? cell.textContent : "").replace(/[^0-9-]/g, ""), 10);
      values[i] = isNaN(n) ? 0 : n;
      if (values[i] > max) {
        max = values[i];
      }
    }
    for (var j = 0; j < slots.length; j++) {
      if (max <= 0 || values[j] <= 0) {
        slots[j].removeAttribute("data-ratio");
        continue; // sıfır değer çubuk basmaz: 2px hayalet "bir şey var" gibi okunur
      }
      var pct = Math.round((values[j] / max) * 20) * 5;
      var fill = document.createElement("span");
      fill.className = "meter-fill v-" + pct;
      slots[j].appendChild(fill);
    }
  }

  // Departman ağacı: alt dalı olan satıra aç/kapa oku basar, kapalı anahtarları
  // localStorage'da tutar. Derinlik `data-depth`ten gelir; bir satırın altları
  // kendisinden derin olan ardışık satırlardır (liste zaten ön sıralı geliyor).
  var TREE_KEY = "opensicil-tree-closed";

  function treeClosed() {
    try {
      return JSON.parse(localStorage.getItem(TREE_KEY)) || {};
    } catch (e) {
      return {};
    }
  }

  function tree() {
    var table = document.getElementById("dept-tree");
    if (!table) {
      return;
    }
    var rows = [].slice.call(table.querySelectorAll("tbody tr[data-depth]"));
    var closed = treeClosed();

    function depth(row) {
      return parseInt(row.getAttribute("data-depth"), 10) || 0;
    }
    function children(index) {
      var out = [];
      for (var i = index + 1; i < rows.length && depth(rows[i]) > depth(rows[index]); i++) {
        out.push(rows[i]);
      }
      return out;
    }
    function paint() {
      var hiddenUntil = -1;
      for (var i = 0; i < rows.length; i++) {
        var d = depth(rows[i]);
        if (hiddenUntil >= 0 && d > hiddenUntil) {
          rows[i].hidden = true;
          continue;
        }
        hiddenUntil = -1;
        rows[i].hidden = false;
        if (closed[rows[i].getAttribute("data-key")]) {
          hiddenUntil = d;
        }
      }
    }

    for (var i = 0; i < rows.length; i++) {
      if (!children(i).length) {
        continue;
      }
      var slot = rows[i].querySelector("[data-tree-slot]");
      if (!slot) {
        continue;
      }
      var button = document.createElement("button");
      button.type = "button";
      button.className = "tree-toggle";
      button.setAttribute("data-key", rows[i].getAttribute("data-key"));
      button.setAttribute("aria-expanded", closed[button.getAttribute("data-key")] ? "false" : "true");
      button.addEventListener("click", function () {
        var key = this.getAttribute("data-key");
        closed[key] = !closed[key];
        this.setAttribute("aria-expanded", closed[key] ? "false" : "true");
        try {
          localStorage.setItem(TREE_KEY, JSON.stringify(closed));
        } catch (e) {
          // saklanamadı: açık/kapalı durumu yalnızca bu sayfa için geçerli
        }
        paint();
      });
      slot.parentNode.replaceChild(button, slot);
    }
    paint();

    // Ağaçta arama: eşleşen satır ve bütün üstleri kalır, gerisi gizlenir.
    var filter = document.querySelector('[data-tree-filter="' + table.id + '"]');
    if (!filter) {
      return;
    }
    filter.addEventListener("input", function () {
      var needle = filter.value.trim().toLocaleLowerCase("tr");
      if (!needle) {
        for (var i = 0; i < rows.length; i++) {
          rows[i].hidden = false;
        }
        paint();
        return;
      }
      var keep = [];
      for (var i = 0; i < rows.length; i++) {
        keep[i] = rows[i].textContent.toLocaleLowerCase("tr").indexOf(needle) >= 0;
      }
      // Eslesen satirin atalari da kalir, yoksa dal koksuz gorunur
      for (var i = rows.length - 1; i >= 0; i--) {
        if (!keep[i]) {
          continue;
        }
        var d = depth(rows[i]);
        for (var up = i - 1; up >= 0 && d > 1; up--) {
          if (depth(rows[up]) < d) {
            keep[up] = true;
            d = depth(rows[up]);
          }
        }
      }
      for (var i = 0; i < rows.length; i++) {
        rows[i].hidden = !keep[i];
      }
    });
  }

  // CSV içe aktarma (F-17): dosya tarayıcıda okunur ve data-csv-into ile adı verilen
  // metin alanına konur; sunucuya sıradan form gider, multipart yolu açılmaz.
  // Dosya seçilemeyen tarayıcıda alan elle yapıştırmaya açık kalır.
  function csvFileInputs() {
    var inputs = document.querySelectorAll("input[type=file][data-csv-into]");
    for (var i = 0; i < inputs.length; i++) {
      bindCsvFile(inputs[i]);
    }
  }

  function bindCsvFile(input) {
    var form = input.form || input.closest("form");
    var target = form && form.querySelector('textarea[name="' + input.getAttribute("data-csv-into") + '"]');
    if (!target || typeof FileReader === "undefined") {
      return;
    }
    input.addEventListener("change", function () {
      var file = input.files && input.files[0];
      if (!file) {
        return;
      }
      var reader = new FileReader();
      reader.onload = function () {
        target.value = String(reader.result);
      };
      reader.readAsText(file, "UTF-8");
    });
  }

  apply(stored());
  navApply(navStored() || navDefault());

  document.addEventListener("DOMContentLoaded", function () {
    themeToggle();
    topbarHeight();
    activeNav();
    navCollapse();
    navDrawer();
    clickableRows();
    dimZeros();
    avatarColors();
    ratioBars();
    tree();
    selectAll();
    csvFileInputs();
  });
})();
