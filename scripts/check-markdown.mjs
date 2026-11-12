// AI writes replies in Markdown; the composer shows its bold, italics and lists as formatting.
import assert from "node:assert/strict";
import { hasStyling, textAsHtml } from "../app/ui/markdown.ts";

assert.equal(
  textAsHtml("*Choosing a Security Company*"),
  "<p><em>Choosing a Security Company</em></p>",
);
assert.equal(textAsHtml("Hi **Sam**, thanks"), "<p>Hi <strong>Sam</strong>, thanks</p>");
assert.equal(
  textAsHtml("Two articles:\n- one\n* two\n• three\nBest,"),
  "<p>Two articles:</p><ul><li><p>one</p></li><li><p>two</p></li><li><p>three</p></li></ul><p>Best,</p>",
);
assert.equal(textAsHtml("1. a\n2) b"), "<ol><li><p>a</p></li><li><p>b</p></li></ol>");
// Plain text stays as it was: lines, blank lines, and asterisks that aren't styling.
assert.equal(
  textAsHtml("5 * 3 and 5*3*2, a <b> tag\n\nBye"),
  "<p>5 * 3 and 5*3*2, a &lt;b&gt; tag</p><p></p><p>Bye</p>",
);
assert.equal(hasStyling("Hi Sam,\n\nSee you then."), false);
assert.equal(hasStyling("- one\n- two"), true);
console.log("markdown ok");
