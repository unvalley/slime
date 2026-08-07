import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const index = readFileSync(resolve(root, "dist/index.html"), "utf8");
const styles = readFileSync(resolve(root, "dist/styles-20260807-15.css"), "utf8");
const buttonScript = readFileSync(resolve(root, "dist/button-20260807-1.js"), "utf8");
const wrangler = readFileSync(resolve(root, "wrangler.jsonc"), "utf8");
const productionCheckoutURL =
  "https://buy.polar.sh/polar_cl_asAsYJgLTkAhiius7JmEbgTIPRpEb4r4H8Unz2x9us2";

const checks = [
  [index.includes("14日間の無料トライアル"), "14-day trial copy"],
  [index.includes("月額200円"), "monthly 200 JPY copy"],
  [index.includes(`href="${productionCheckoutURL}"`) && !index.includes("__SLIME_POLAR_CHECKOUT_URL__"), "production checkout URL"],
  [index.includes('rel="canonical" href="https://slime.unvalley.me/"'), "canonical URL"],
  [wrangler.includes('"pattern": "slime.unvalley.me"'), "Cloudflare custom domain"],
  [index.includes('class="skip-link" href="#main"'), "skip navigation link"],
  [index.includes("<h1>Macのための新しいIME</h1>") && !index.includes("考える速さで"), "single-line hero copy"],
  [index.includes('<span class="action-label">ダウンロード</span>'), "download CTA copy"],
  [index.includes('<p class="lead">ライブ変換・履歴補完・ローカル完結。</p>'), "concise product copy"],
  [!index.includes("ライブ変換と履歴補完を備えた") && !index.includes("変換も学習も、このMacで完結します"), "old product copy removed"],
  [!index.includes("macOS 13以降"), "platform requirement removed"],
  [!index.includes('class="site-header"') && index.includes('class="hero"') && !index.includes('class="feature-list"'), "minimal centered structure"],
  [!index.includes("<nav") && !index.includes("<details") && !index.includes("<article"), "nonessential sections removed"],
  [!index.includes("<img") && !index.includes("<figure") && !styles.includes(".writing-shot"), "image-free landing"],
  [!index.includes("slime-settings.png") && !existsSync(resolve(root, "dist/slime-settings.png")), "settings screenshot excluded"],
  [!index.includes("Slime Playground") && !index.includes("section-label") && !index.includes("final-section"), "generated landing tropes removed"],
  [index.includes('src="/button-20260807-1.js?v=202608070145"'), "versioned button interaction"],
  [index.includes('href="/styles-20260807-15.css?v=202608070205"'), "versioned stylesheet URL"],
  [!index.includes("brand-mark") && !styles.includes(".brand-mark"), "Slime brand icon removed"],
  [index.includes("<p>© 2026 unvalley</p>") && !index.includes("ユーザー辞書の移行に対応"), "migration footer copy removed"],
  [index.includes('<meta name="theme-color" content="#fafaf8"'), "light browser theme"],
  [styles.includes("color-scheme: light") && styles.includes("--paper: #fafaf8"), "light full-page palette"],
  [styles.includes("flex-direction: column") && styles.includes("text-align: center"), "centered hero layout"],
  [!styles.includes(".feature-list"), "feature grid styles removed"],
  [!styles.includes("border-radius: 20px") && !styles.includes("--outside:"), "Atlas card structure removed"],
  [styles.includes("font-size: clamp(40px, 3.7vw, 48px)") && styles.includes("white-space: nowrap"), "single-line hero type"],
  [!/(?:font-weight:\s*(?:[6-9]\d{2}|bold|bolder))|<(?:strong|b)\b/i.test(`${styles}\n${index}`), "bold typography prohibited"],
  [styles.includes('font-feature-settings: "palt" 1, "kern" 1') && styles.includes("line-break: strict"), "Japanese typography metrics"],
  [styles.includes("font-variant-numeric: tabular-nums"), "price typography"],
  [styles.includes("prefers-reduced-motion"), "reduced-motion behavior"],
  [styles.includes("radial-gradient(") && styles.includes("--pointer-x") && buttonScript.includes('style.setProperty("--pointer-x"'), "pointer-following CTA glow"],
  [styles.includes(".action-icon-mac") && styles.includes(".action-icon-arrow"), "CTA icon transition"],
  [!styles.includes("transition: all") && !styles.includes("@import"), "specific local styling"],
];

for (const [passed, name] of checks) {
  if (!passed) throw new Error(`Landing check failed: ${name}`);
}

console.log(`Landing checks passed (${checks.length}).`);
