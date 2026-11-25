import { parse } from "@babel/parser";

const METADATA = new Set([
  "loc",
  "start",
  "end",
  "range",
  "extra",
  "leadingComments",
  "trailingComments",
  "innerComments",
  "comments",
  "tokens",
]);

/** Parse JavaScript, CommonJS, TypeScript or JSX into Babel's program node. */
export function parseSource(file, content) {
  const plugins = [
    /\.[cm]?tsx?$/.test(file) && "typescript",
    /\.[jt]sx$/.test(file) && "jsx",
  ].filter(Boolean);
  return parse(content, { sourceType: "unambiguous", sourceFilename: file, plugins }).program;
}

/** Whether a node is an ordinary or optional function call. */
export function isCall(node) {
  return node?.type === "CallExpression" || node?.type === "OptionalCallExpression";
}

/** Whether a node is an ordinary or optional property access. */
export function isMember(node) {
  return node?.type === "MemberExpression" || node?.type === "OptionalMemberExpression";
}

/** Visit each AST child, excluding source locations and comment metadata. */
export function forEachChild(node, visit) {
  for (const [key, value] of Object.entries(node)) {
    if (METADATA.has(key)) continue;
    if (Array.isArray(value)) {
      for (const child of value) {
        if (child !== null && typeof child === "object" && typeof child.type === "string") {
          visit(child);
        }
      }
    } else if (value !== null && typeof value === "object" && typeof value.type === "string") {
      visit(value);
    }
  }
}

/** Walk a complete AST in source order. */
export function walk(node, visit) {
  visit(node);
  forEachChild(node, (child) => walk(child, visit));
}

/** The exact source text one node spans. */
export function nodeText(node, content) {
  return content.slice(node.start, node.end);
}
