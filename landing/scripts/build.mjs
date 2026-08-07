import { cpSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const landingRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const sourceRoot = resolve(landingRoot, "src");
const outputRoot = resolve(landingRoot, "dist");
const productionCheckoutURL =
  "https://buy.polar.sh/polar_cl_asAsYJgLTkAhiius7JmEbgTIPRpEb4r4H8Unz2x9us2";
const checkoutValue =
  process.env.SLIME_POLAR_CHECKOUT_URL?.trim() || productionCheckoutURL;

const checkoutURL = new URL(checkoutValue);
if (checkoutURL.protocol !== "https:") {
  throw new Error("SLIME_POLAR_CHECKOUT_URL must be an HTTPS URL.");
}

rmSync(outputRoot, { recursive: true, force: true });
mkdirSync(outputRoot, { recursive: true });
cpSync(sourceRoot, outputRoot, { recursive: true });
renameSync(resolve(outputRoot, "styles.css"), resolve(outputRoot, "styles-20260807-15.css"));
renameSync(resolve(outputRoot, "button.js"), resolve(outputRoot, "button-20260807-1.js"));
rmSync(resolve(outputRoot, "slime-settings.png"), { force: true });

const indexPath = resolve(outputRoot, "index.html");
const index = readFileSync(indexPath, "utf8").replaceAll(
  "__SLIME_POLAR_CHECKOUT_URL__",
  checkoutURL.href,
);
writeFileSync(indexPath, index);

console.log(`Built ${outputRoot}`);
