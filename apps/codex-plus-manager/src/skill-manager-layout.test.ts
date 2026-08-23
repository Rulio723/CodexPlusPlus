import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("./styles.css", import.meta.url), "utf8");

test("Skills manager keeps filters and entries in a bounded responsive layout", () => {
  for (const selector of [
    ".skill-manager",
    ".skill-toolbar",
    ".skill-search",
    ".skill-metrics",
    ".skill-list",
    ".skill-row",
    ".skill-main",
    ".skill-title-row",
    ".skill-actions",
  ]) {
    assert.match(styles, new RegExp(`\\${selector}\\s*\\{`), `${selector} must have explicit layout styles`);
  }

  assert.match(styles, /\.skill-toolbar\s*\{[^}]*grid-template-columns:[^}]*minmax\(0,\s*1fr\)/s);
  assert.match(styles, /\.skill-row\s*\{[^}]*grid-template-columns:[^}]*minmax\(0,\s*1fr\)[^}]*auto/s);
  assert.match(styles, /\.skill-main p\s*\{[^}]*-webkit-line-clamp:\s*2/s);
  assert.match(styles, /\.skill-main code\s*\{[^}]*text-overflow:\s*ellipsis/s);
  assert.match(styles, /@media[^}]*max-width:\s*900px[\s\S]*?\.skill-row\s*\{[^}]*grid-template-columns:\s*minmax\(0,\s*1fr\)/s);

  assert.match(app, /<p title=\{skill\.valid \? skill\.description : skill\.error/);
  assert.match(app, /className="skill-invocation"/);
});
