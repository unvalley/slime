import { cpSync, mkdirSync, renameSync, rmSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const landingRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const sourceRoot = resolve(landingRoot, "src");
const outputRoot = resolve(landingRoot, "dist");
rmSync(outputRoot, { recursive: true, force: true });
mkdirSync(outputRoot, { recursive: true });
cpSync(sourceRoot, outputRoot, { recursive: true });
renameSync(resolve(outputRoot, "styles.css"), resolve(outputRoot, "styles-20260807-15.css"));
renameSync(resolve(outputRoot, "button.js"), resolve(outputRoot, "button-20260807-1.js"));
rmSync(resolve(outputRoot, "slime-settings.png"), { force: true });

console.log(`Built ${outputRoot}`);
