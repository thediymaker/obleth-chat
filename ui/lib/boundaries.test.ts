// lib/ holds what pages share without drawing anything: it may not import from
// components/, so the store, the bridges and the helpers can be used and tested alone.

import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { expect, test } from "vitest";

const LIB = join(__dirname);

/** The lines of a source file that import from components/. */
function componentImports(source: string) {
  return source.split("\n").filter((line) => /\bfrom\s+["']@\/components\/|\bimport\(\s*["']@\/components\/|^\s*import\s+["']@\/components\//.test(line));
}

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? sources(join(dir, e.name)) : /\.tsx?$/.test(e.name) ? [join(dir, e.name)] : []));
}

test("the check finds an import from components", () => {
  expect(componentImports('import { Pill } from "@/components/manage/kit";\nimport { api } from "@/lib/api";')).toHaveLength(1);
  expect(componentImports('const kit = await import("@/components/manage/kit");')).toHaveLength(1);
  expect(componentImports('import { api } from "@/lib/api";')).toHaveLength(0);
});

test("nothing in lib imports from components", () => {
  const found = sources(LIB)
    .filter((file) => file !== __filename)
    .flatMap((file) => componentImports(readFileSync(file, "utf8")).map((line) => `${relative(LIB, file)}: ${line.trim()}`));
  expect(found).toEqual([]);
});
