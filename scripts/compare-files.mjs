import { readFile } from "node:fs/promises";

const [expectedPath, actualPath] = process.argv.slice(2);

if (!expectedPath || !actualPath) {
  console.error("Usage: node scripts/compare-files.mjs <expected> <actual>");
  process.exit(2);
}

const [expected, actual] = await Promise.all([
  readFile(expectedPath, "utf8"),
  readFile(actualPath, "utf8"),
]);

if (expected !== actual) {
  console.error(
    `OpenAPI client drift detected: ${expectedPath} differs from ${actualPath}. Run \`pnpm --dir apps/web api:generate\`.`,
  );
  process.exit(1);
}

console.log("OpenAPI schema/client drift check: OK");
