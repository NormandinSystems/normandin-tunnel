import { describe, expect, it } from "vitest";

import en from "./en.json";
import fr from "./fr.json";

type Tree = { [key: string]: string | Tree };

function keys(tree: Tree, prefix = ""): string[] {
  return Object.entries(tree).flatMap(([key, value]) =>
    typeof value === "string" ? [`${prefix}${key}`] : keys(value, `${prefix}${key}.`),
  );
}

function placeholders(text: string): string[] {
  return (text.match(/\{\{\w+\}\}/g) ?? []).sort();
}

function lookup(tree: Tree, path: string): string {
  return path.split(".").reduce<string | Tree>((node, part) => (node as Tree)[part], tree) as string;
}

describe("translations", () => {
  it("has a French string for every English string, and nothing extra", () => {
    expect(keys(fr).sort()).toEqual(keys(en).sort());
  });

  it("keeps the same placeholders in both languages", () => {
    for (const key of keys(en)) {
      expect(placeholders(lookup(fr, key)), key).toEqual(placeholders(lookup(en, key)));
    }
  });

  it("has no empty strings", () => {
    for (const key of keys(en)) {
      expect(lookup(en, key).trim(), key).not.toBe("");
      expect(lookup(fr, key).trim(), key).not.toBe("");
    }
  });

  it("uses plain language: no protocol jargon on screen", () => {
    const jargon = /\b(PAKE|SPAKE|QUIC|endpoint|nameplate|rendezvous|BLAKE3|HMAC)\b/i;
    for (const key of keys(en)) {
      expect(jargon.test(lookup(en, key)), `${key}: ${lookup(en, key)}`).toBe(false);
    }
  });
});
