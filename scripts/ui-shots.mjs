// Arayuzun sayfalarini iki temada ve iki genislikte goruntuler (ADR-114 dogrulamasi).
// Calistirma:  sh scripts/ui-shots.sh <cikti-dizini>   (betik bagimliligi kurar)
//
// Kimlik: OPENSICIL_USER / OPENSICIL_PASS ortam degiskenleri ya da `tmp/.e2e-env`
// dosyasi (KEY=DEGER satirlari, git'e girmez). Parola komut satirina yazilmaz.
//
// Goruntunun yaninda olcum yapar, cunku goze bakmak hepsini kaciriyor:
//   1. yatay tasma  : documentElement.scrollWidth > innerWidth
//   2. tofu kutusu  : .ico'nun hesaplanmis font-family'si Nerd Font'u icermiyor
//   3. govde fontu  : body'nin hesaplanmis font-family'si Nerd Font'u icermiyor
//   4. CSP          : konsola dusen "Content Security Policy" satirlari
//   5. ic kaydirma  : bir tablo kendi kutusunda dikey kayiyor (sayfa kaymali)
//   6. renk sayisi  : panelde sayac karti 4 renk; etkinlik ikonunun renk
//                     sayisi akistaki ayri **kategori** sayisindan az olmamali
//                     (olay turu degil: bes kategori var, olay turu onlarca —
//                     ayni kategorideki iki olayin ayni renkte olmasi dogru.
//                     Olayin kategorisiz kalmasini `dashboard.rs` testi yakalar)
//   7. yapiskan ofset: ic kaydirmali kutudaki yapiskan baslik `top: 0` olmali,
//                     yoksa baslik govdenin icine iner
//   8. bos baslik   : metni ve kontrolu olmayan <th> (kolon adsiz kaliyor)
//   9. beyaz kenar  : kart kenarligi neredeyse beyaz ve opak — tanimsiz renk
//                     degiskeni `currentColor`a dusunce boyle cikiyor
//  10. kirilan dugme: .btn tek satira sigmamis (etiket iki satira sarmis)
// `createRequire`: playwright tmp/araclar/pw altinda durdugu icin ESM'in bare
// specifier cozumu onu bulamiyor (NODE_PATH yalnizca require'da gecerli).
import { createRequire } from "node:module";
import { mkdir, readFile, writeFile } from "node:fs/promises";
const { chromium } = createRequire(import.meta.url)("playwright");
import path from "node:path";

const BASE = process.env.OPENSICIL_BASE ?? "https://192.168.1.112";
const OUT = process.argv[2];
if (!OUT) {
  console.error("kullanim: node scripts/ui-shots.mjs <cikti-dizini>");
  process.exit(2);
}

// tmp/.e2e-env dosyasindan okunur; ortam degiskeni varsa o kazanir.
async function creds() {
  const env = { user: process.env.OPENSICIL_USER, pass: process.env.OPENSICIL_PASS };
  if (env.user && env.pass) return env;
  try {
    const text = await readFile("tmp/.e2e-env", "utf8");
    for (const line of text.split("\n")) {
      const m = line.match(/^\s*(OPENSICIL_USER|OPENSICIL_PASS)\s*=\s*(.*?)\s*$/);
      if (!m) continue;
      if (m[1] === "OPENSICIL_USER") env.user ??= m[2];
      else env.pass ??= m[2];
    }
  } catch {
    // dosya yok: asagida hata verilir
  }
  if (!env.user || !env.pass) {
    console.error("OPENSICIL_USER / OPENSICIL_PASS yok (ortam ya da tmp/.e2e-env)");
    process.exit(2);
  }
  return env;
}

const FIXED_PAGES = [
  ["panel", "/"],
  ["personel", "/identities"],
  ["roller", "/roles"],
  ["departmanlar", "/departments"],
  ["mutabakat", "/reconcile"],
  ["raporlar", "/reports"],
  ["uygulamalar", "/targets"],
  ["ayarlar", "/config"],
  ["ice-aktarma", "/imports"],
];
const THEMES = ["koyu", "acik"];
const WIDTHS = [
  ["1440", 1440, 900],
  ["390", 390, 844],
];

// Kimlik / hedef / rol / departman adresleri veriden gelir: liste sayfasindaki
// ilk baglantidan okunur, boylece betikte id ve slug sabitlenmez.
async function detailRoutes(page) {
  const found = [];
  const first = async (listPath, selector) => {
    await page.goto(BASE + listPath, { waitUntil: "domcontentloaded" });
    return page.evaluate((sel) => {
      const link = document.querySelector(sel);
      return link ? new URL(link.href).pathname : null;
    }, selector);
  };
  const kisi = await first("/identities", '.tbl tbody a[href^="/identities/"]');
  if (kisi) found.push(["kisi", kisi]);
  // Roller sayfasi kart izgarasi (ADR-119 A.2): baglanti artik tabloda degil
  const rol = await first("/roles", '.role-grid a[href^="/roles/"], .tbl tbody a[href^="/roles/"]');
  if (rol) found.push(["rol", rol]);
  const departman = await first("/departments", '.tbl tbody a[href^="/departments/"]');
  if (departman) found.push(["departman", departman]);

  // Hedeflerin hepsi: mutabakat ve esleme sayfalari hedef basina ayri cizilir
  await page.goto(`${BASE}/targets`, { waitUntil: "domcontentloaded" });
  // Secici hedef basina baglantiyi arar; kenar cubugundaki "Mutabakat" maddesi
  // de `/reconcile` ile bitiyor (ADR-123) ve ona takilirsa id `undefined` olur
  const targets = await page.evaluate(() =>
    [...document.querySelectorAll('a[href^="/targets/"][href$="/reconcile"]')].map(
      (a) => new URL(a.href).pathname.split("/")[2]
    )
  );
  for (const id of targets) {
    found.push([`mutabakat-${id}`, `/targets/${id}/reconcile`]);
    found.push([`eslemeler-${id}`, `/targets/${id}/mappings`]);
  }
  return found;
}

// Sayfanin icinden olculenler. Tarayici baglaminda calisir, DOM disina cikmaz.
function probePage() {
  const doc = document.documentElement;
  const ico = document.querySelector(".ico");
  const colors = (sel) => {
    const seen = new Set();
    for (const el of document.querySelectorAll(sel)) seen.add(getComputedStyle(el).color);
    return seen.size;
  };
  const label = (el) => (el.textContent || "").trim().slice(0, 40) || el.className;

  // Ic kaydirma: kutu icerikten kisa kalmis demektir, sayfa degil tablo kayar
  const inner = [];
  for (const box of document.querySelectorAll(".tbl-scroll, .feed, .card")) {
    // `max-height` verilmis kutu bilerek kendi icinde kayar (sahiplenme listesi)
    if (getComputedStyle(box).maxHeight !== "none") continue;
    if (box.scrollHeight > box.clientHeight + 1) inner.push(box.className.split(" ")[0]);
  }

  // Yapiskan baslik, kendi kaydirma kutusunun tepesine oturmali
  const scrolls = (el) => {
    const s = getComputedStyle(el);
    return s.overflowX !== "visible" || s.overflowY !== "visible";
  };
  const stickyOffset = [];
  for (const el of document.querySelectorAll("th, .sticky, [class*='sticky']")) {
    if (getComputedStyle(el).position !== "sticky") continue;
    const top = getComputedStyle(el).top;
    if (top === "0px" || top === "auto") continue;
    for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
      if (scrolls(p)) {
        stickyOffset.push(`${label(el)} @ .${p.className.split(" ")[0]} top=${top}`);
        break;
      }
    }
  }

  // Bos kolon basligi: metni de kontrolu de yoksa kolonun adi yok demektir
  const emptyTh = [...document.querySelectorAll("th")]
    .filter((th) => !th.textContent.trim() && !th.querySelector("input, button, a, svg, i"))
    .map((th) => `tablo#${[...document.querySelectorAll("table")].indexOf(th.closest("table"))}`);

  // Beyaz kenarlikli kart: tanimsiz renk degiskeni `currentColor`a duser
  const whiteBorder = [];
  for (const el of document.querySelectorAll(".card, .card-tight, .stat, .panel")) {
    const m = getComputedStyle(el).borderTopColor.match(/[\d.]+/g);
    if (!m) continue;
    const [r, g, b] = m.map(Number);
    const a = m.length > 3 ? Number(m[3]) : 1;
    if (a >= 0.5 && Math.min(r, g, b) >= 200) whiteBorder.push(label(el));
  }

  // Iki satira kirilan dugme: ic yukseklik tek satirin sinirini asmis
  const wrappedBtn = [];
  for (const el of document.querySelectorAll(".btn")) {
    const s = getComputedStyle(el);
    const inner =
      el.clientHeight - parseFloat(s.paddingTop || 0) - parseFloat(s.paddingBottom || 0);
    if (inner > parseFloat(s.fontSize) * 1.9) wrappedBtn.push(label(el));
  }

  return {
    scrollWidth: doc.scrollWidth,
    innerWidth: window.innerWidth,
    icoFont: ico ? getComputedStyle(ico).fontFamily : "(.ico yok)",
    bodyFont: getComputedStyle(document.body).fontFamily,
    feedColors: colors(".feed .feed-ico"),
    // Akistaki ayri olay turu: renk sayisi bunun altinda kalirsa
    // kategorilendirme bozulmus demektir
    feedKinds: new Set(
      [...document.querySelectorAll(".feed .feed-row")].map(
        (e) => (e.className.match(/feed-row--\w+/) || ["?"])[0]
      )
    ).size,
    statColors: colors(".stat .stat-value"),
    innerScroll: inner.join(","),
    stickyOffset: [...new Set(stickyOffset)],
    emptyTh: [...new Set(emptyTh)],
    whiteBorder: [...new Set(whiteBorder)],
    wrappedBtn: [...new Set(wrappedBtn)],
  };
}

const { user, pass } = await creds();
await mkdir(OUT, { recursive: true });

const browser = await chromium.launch();
const findings = [];
let pages = null;

for (const [themeName, themeValue] of [
  ["koyu", "dark"],
  ["acik", "light"],
].filter(([n]) => THEMES.includes(n))) {
  for (const [widthName, width, height] of WIDTHS) {
    const context = await browser.newContext({
      ignoreHTTPSErrors: true,
      viewport: { width, height },
      deviceScaleFactor: 1,
    });
    // Tema secimi app.js'in okudugu localStorage anahtari; her sayfadan once kurulur
    await context.addInitScript(
      `try { localStorage.setItem("opensicil-theme", ${JSON.stringify(themeValue)}); } catch (e) {}`
    );
    const page = await context.newPage();

    // Konsol: CSP ihlali sessizdir, yalnizca burada gorunur
    const console_errors = [];
    page.on("console", (m) => {
      if (m.type() === "error") console_errors.push(m.text());
    });
    page.on("pageerror", (e) => console_errors.push(String(e)));

    await page.goto(`${BASE}/login`, { waitUntil: "domcontentloaded" });
    await page.fill("#username", user);
    await page.fill("#password", pass);
    await page.click('form[action="/login"] button[type=submit]');
    await page.waitForLoadState("domcontentloaded");
    // Basarili giris ana sayfaya yonlendirir (POST/Redirect/GET). Oturumun
    // kurulup kurulmadigi korumali bir sayfa istenerek anlasilir — oturum
    // yoksa `/login`e geri atilir.
    await page.goto(`${BASE}/`, { waitUntil: "domcontentloaded" });
    if (new URL(page.url()).pathname === "/login") {
      console.error("giris basarisiz: kimlik yanlis ya da hesap kilitli");
      process.exit(1);
    }
    pages ??= [...FIXED_PAGES, ...(await detailRoutes(page))];

    for (const [name, route] of pages) {
      await page.goto(BASE + route, { waitUntil: "networkidle" });
      const file = path.join(OUT, `${name}-${themeName}-${widthName}.png`);
      await page.screenshot({ path: file, fullPage: true });

      const probe = await page.evaluate(probePage);
      const tag = `${name}-${themeName}-${widthName}`;
      if (probe.scrollWidth > probe.innerWidth + 1) {
        findings.push(`TASMA   ${tag}: scrollWidth ${probe.scrollWidth} > ${probe.innerWidth}`);
      }
      if (!/CaskaydiaMono/.test(probe.icoFont)) {
        findings.push(`TOFU    ${tag}: .ico font-family = ${probe.icoFont}`);
      }
      if (!/CaskaydiaMono/.test(probe.bodyFont)) {
        findings.push(`FONT    ${tag}: body font-family = ${probe.bodyFont}`);
      }
      if (probe.innerScroll) {
        findings.push(`IC-KAYDIRMA ${tag}: ${probe.innerScroll}`);
      }
      if (probe.stickyOffset.length) {
        findings.push(`YAPISKAN ${tag}: ${probe.stickyOffset.join(" | ")}`);
      }
      if (probe.emptyTh.length) {
        findings.push(`BOS-TH  ${tag}: ${probe.emptyTh.join(", ")}`);
      }
      if (probe.whiteBorder.length) {
        findings.push(`BEYAZ-KENAR ${tag}: ${probe.whiteBorder.join(" | ")}`);
      }
      if (probe.wrappedBtn.length) {
        findings.push(`KIRIK-DUGME ${tag}: ${probe.wrappedBtn.join(" | ")}`);
      }
      if (name === "panel" && probe.feedColors < probe.feedKinds) {
        findings.push(
          `TEK-RENK ${tag}: etkinlik ikonu ${probe.feedColors} renk, ${probe.feedKinds} ayri kategori`
        );
      }
      if (name === "panel" && probe.statColors !== 4) {
        findings.push(`TEK-RENK ${tag}: sayac karti ${probe.statColors} renk (4 olmali)`);
      }
      const csp = console_errors.filter((t) => /Content Security Policy/i.test(t));
      if (csp.length) {
        findings.push(`CSP     ${tag}: ${csp[0]}`);
      }
      console_errors.length = 0;
      console.log(
        `${tag}  ${probe.scrollWidth}/${probe.innerWidth}px  akis:${probe.feedColors}/${probe.feedKinds} sayac:${probe.statColors}`
      );
    }
    await context.close();
  }
}

await browser.close();
const report = findings.length ? findings.join("\n") : "bulgu yok";
await writeFile(path.join(OUT, "olcum.txt"), report + "\n");
console.log("\n" + report);
process.exit(findings.length ? 1 : 0);
