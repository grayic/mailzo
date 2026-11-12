// AI writes in Markdown. The composer shows its bold, italics and lists as formatting.
// No imports, so scripts/check-markdown.mjs can run it with plain Node.

const escape = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** **bold** and *italics*. An asterisk with a space or a letter beside it, as in 2 * 3 or 5*3, stays. */
const inline = (s: string) =>
  escape(s)
    .replace(/(?<![\w*])\*\*(?=\S)(.+?)(?<=\S)\*\*(?![\w*])/g, "<strong>$1</strong>")
    .replace(/(?<![\w*])\*(?=\S)(.+?)(?<=\S)\*(?![\w*])/g, "<em>$1</em>");

const BULLET = /^\s*[-*•]\s+(.*)$/;
const NUMBERED = /^\s*\d+[.)]\s+(.*)$/;

/** Text as the editor's lines: each line a paragraph, blank lines kept, "- " and "1. " lines as lists. */
export function textAsHtml(text: string): string {
  let html = "";
  let open = "";
  for (const line of text.split("\n")) {
    const bullet = BULLET.exec(line);
    const numbered = bullet ? null : NUMBERED.exec(line);
    const list = bullet ? "ul" : numbered ? "ol" : "";
    if (list !== open) {
      if (open) html += `</${open}>`;
      if (list) html += `<${list}>`;
      open = list;
    }
    const item = bullet ?? numbered;
    html += item ? `<li><p>${inline(item[1])}</p></li>` : `<p>${inline(line)}</p>`;
  }
  return open ? `${html}</${open}>` : html;
}

/** Whether the text has any bold, italics or lists. */
export const hasStyling = (text: string) => /<(strong|em|li)>/.test(textAsHtml(text));
