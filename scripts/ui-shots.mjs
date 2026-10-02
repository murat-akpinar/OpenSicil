// Arayuzun yedi sayfasini iki temada ve iki genislikte goruntuler (ADR-114 dogrulamasi).
// Calistirma:  sh scripts/ui-shots.sh <cikti-dizini>   (betik bagimliligi kurar)
//
// Kimlik: OPENSICIL_USER / OPENSICIL_PASS ortam degiskenleri ya da `tmp/.e2e-env`
// dosyasi (KEY=DEGER satirlari, git'e girmez). Parola komut satirina yazilmaz.
//
// Goruntunun yaninda iki olcum yapar, cunku goze bakmak ikisini kaciriyor:
//   1. yatay tasma  : documentElement.scrollWidth > innerWidth
//   2. tofu kutusu  : .ico'nun hesaplanmis font-family'si Nerd Font'u icermiyor
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

const PAGES = [
  ["panel", "/"],
  ["personel", "/identities"],
  ["roller", "/roles"],
  ["departmanlar", "/departments"],
  ["raporlar", "/reports"],
  ["uygulamalar", "/targets"],
  ["ayarlar", "/config"],
];
const THEMES = ["koyu", "acik"];
const WIDTHS = [["1440", 1440, 900], ["390", 390, 844]];

const { user, pass } = await creds();
await mkdir(OUT, { recursive: true });

const browser = await chromium.launch();
const findings = [];

for (const [themeName, themeValue] of [["koyu", "dark"], ["acik", "light"]].filter(([n]) => THEMES.includes(n))) {
  for (const [widthName, width, height] of WIDTHS) {
    const context = await browser.newContext({
      ignoreHTTPSErrors: true,
      viewport: { width, height },
      deviceScaleFactor: 1,
    });
    // Tema secimi app.js'in okudugu localStorage anahtari; her sayfadan once kurulur
    await context.addInitScript(`try { localStorage.setItem("opensicil-theme", ${JSON.stringify(themeValue)}); } catch (e) {}`);
    const page = await context.newPage();

    await page.goto(`${BASE}/login`, { waitUntil: "domcontentloaded" });
    await page.fill("#username", user);
    await page.fill("#password", pass);
    await page.click('form[action="/login"] button[type=submit]');
    await page.waitForLoadState("domcontentloaded");
    if (new URL(page.url()).pathname === "/login") {
      console.error("giris basarisiz: kimlik yanlis ya da hesap kilitli");
      process.exit(1);
    }

    for (const [name, route] of PAGES) {
      await page.goto(BASE + route, { waitUntil: "networkidle" });
      const file = path.join(OUT, `${name}-${themeName}-${widthName}.png`);
      await page.screenshot({ path: file, fullPage: true });

      const probe = await page.evaluate(() => {
        const doc = document.documentElement;
        const ico = document.querySelector(".ico");
        return {
          scrollWidth: doc.scrollWidth,
          innerWidth: window.innerWidth,
          icoFont: ico ? getComputedStyle(ico).fontFamily : "(.ico yok)",
          bodyFont: getComputedStyle(document.body).fontFamily,
        };
      });
      const tag = `${name}-${themeName}-${widthName}`;
      if (probe.scrollWidth > probe.innerWidth + 1) {
        findings.push(`TASMA   ${tag}: scrollWidth ${probe.scrollWidth} > ${probe.innerWidth}`);
      }
      if (!/CaskaydiaMono/.test(probe.icoFont)) {
        findings.push(`TOFU    ${tag}: .ico font-family = ${probe.icoFont}`);
      }
      console.log(`${tag}  ${probe.scrollWidth}/${probe.innerWidth}px`);
    }
    await context.close();
  }
}

await browser.close();
const report = findings.length ? findings.join("\n") : "tasma yok, tofu yok";
await writeFile(path.join(OUT, "olcum.txt"), report + "\n");
console.log("\n" + report);
process.exit(findings.length ? 1 : 0);
