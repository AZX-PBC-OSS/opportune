#!/usr/bin/env node
// Rewrites the combined schema bundle (read on stdin, written to stdout) into the
// JSON Schema dialect json-schema-to-typescript handles, which keeps the generated
// types one-per-definition instead of one-per-reference-site:
//
// - 2020-12 `$defs` become `definitions`, and `#/$defs/...` refs are rewritten to
//   match, so json2ts sees the layout it names types from.
// - 2020-12 `prefixItems` tuples become draft-07 `items` arrays, which json2ts
//   renders as exact tuples (`[string, string]`) rather than `[unknown, unknown]`.
// - A `$ref` that carries sibling keywords is wrapped as `allOf: [{$ref}, ...]`:
//   json2ts resolves a bare or single-member-allOf `$ref` to the referenced type's
//   name, but inlines a ref with siblings and mints a fresh alias per call site.
// - The `default` sibling of a `$ref` is dropped rather than wrapped: json2ts
//   turns it into a second allOf member and intersects it into the referenced
//   type. No `$ref` in the bundle carries one today — a defaulted field states
//   its shape as `anyOf: [<shape>, {"type": "null"}]` with the `default` beside
//   it, which json2ts renders as the union it is — so this is the guard for a
//   `$ref` that gains the sibling again. The Rust side keeps the default, and no
//   TypeScript type can carry it, so nothing is lost here.
//
// Sibling `description`s are kept: they are what json2ts renders as the field's
// JSDoc comment.
import { stdin } from "node:process";

const text = await new Promise((resolve, reject) => {
  let input = "";
  stdin.setEncoding("utf8");
  stdin.on("data", (chunk) => (input += chunk));
  stdin.on("end", () => resolve(input));
  stdin.on("error", reject);
});

const ref = (value) => ({
  $ref: value.replace("#/$defs/", "#/definitions/"),
});

const convert = (node) => {
  if (Array.isArray(node)) return node.map(convert);
  if (node === null || typeof node !== "object") return node;

  if ("$ref" in node) {
    const siblings = Object.entries(node).filter(
      ([key]) => key !== "$ref" && key !== "default",
    );
    if (siblings.length === 0) return ref(node.$ref);
    return {
      allOf: [ref(node.$ref)],
      ...Object.fromEntries(siblings.map(([key, value]) => [key, convert(value)])),
    };
  }

  const result = {};
  for (const [key, value] of Object.entries(node)) {
    if (key === "$defs") {
      result.definitions = Object.fromEntries(
        Object.entries(value).map(([name, def]) => [name, convert(def)]),
      );
    } else if (key === "prefixItems") {
      result.items = value.map(convert);
    } else {
      result[key] = convert(value);
    }
  }
  return result;
};

process.stdout.write(JSON.stringify(convert(JSON.parse(text)), null, 2) + "\n");
