import type { IconSvgElement } from "@hugeicons/react";
import { useEffect, useState } from "react";
import { Editor, EditorContent, useEditor, useEditorState } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import Image from "@tiptap/extension-image";
import Placeholder from "@tiptap/extension-placeholder";
import TextAlign from "@tiptap/extension-text-align";
import { TextStyleKit } from "@tiptap/extension-text-style";
import { readAsDataUrl } from "../files";
import { textAsHtml } from "../markdown";
import {
  ArrowDown01Icon,
  Image01Icon,
  LeftToRightListBulletIcon,
  LeftToRightListNumberIcon,
  Link01Icon,
  QuoteDownIcon,
  TextAlignCenterIcon,
  TextAlignLeftIcon,
  TextAlignRightIcon,
  TextBoldIcon,
  TextClearIcon,
  TextColorIcon,
  TextFontIcon,
  TextItalicIcon,
  TextStrikethroughIcon,
  TextUnderlineIcon,
} from "../icons";
import { Icon, Menu, MenuItem, Spinner } from "./ui";
import { tx } from "../i18n";

const MAX_INLINE_IMAGE = 5 * 1024 * 1024;

// These become part of the email itself, which other mail apps show without any
// of this app's styles, so they are written out as real sizes and colours.
const SIZES = [
  ["Small", "12px"],
  ["Normal", ""],
  ["Large", "18px"],
  ["Huge", "24px"],
];
const COLOURS = [
  "#17171a", "#6b6b74", "#c8341f", "#d9730d", "#b58a00", "#1a8a56",
  "#0f8a9d", "#2f5bea", "#6741d9", "#b5338a", "#7a4a21", "#ffffff",
];

function Tool({
  icon,
  label,
  active,
  onClick,
}: {
  icon: IconSvgElement;
  label: string;
  active?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className="tool"
      aria-label={label}
      data-tip={label}
      aria-pressed={active ?? false}
      // Keeps the selection in the editor while a button is pressed.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
    >
      <Icon icon={icon} size="s" />
    </button>
  );
}

function Toolbar({ editor, onImage }: { editor: Editor; onImage: (f: File) => void }) {
  const [linking, setLinking] = useState(false);
  const [href, setHref] = useState("");
  const state = useEditorState({
    editor,
    selector: ({ editor: e }) => ({
      bold: e.isActive("bold"),
      italic: e.isActive("italic"),
      underline: e.isActive("underline"),
      strike: e.isActive("strike"),
      bullets: e.isActive("bulletList"),
      numbers: e.isActive("orderedList"),
      quote: e.isActive("blockquote"),
      link: e.isActive("link"),
      center: e.isActive({ textAlign: "center" }),
      right: e.isActive({ textAlign: "right" }),
    }),
  });

  const applyLink = () => {
    const text = href.trim();
    const chain = editor.chain().focus().extendMarkRange("link");
    if (!text) {
      chain.unsetLink().run();
    } else {
      const full = /^(https?:|mailto:)/i.test(text)
        ? text
        : text.includes("@")
          ? `mailto:${text}`
          : `https://${text}`;
      chain.setLink({ href: full }).run();
    }
    setLinking(false);
    setHref("");
  };

  return (
    <>
      {linking && (
        <div className="link-row">
          <Icon icon={Link01Icon} size="s" />
          <input
            autoFocus
            aria-label={tx("Link address")}
            placeholder="https://example.com"
            className="plain-input"
            value={href}
            onChange={(e) => setHref(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                applyLink();
              } else if (e.key === "Escape") {
                e.stopPropagation();
                setLinking(false);
              }
            }}
          />
          <button type="button" className="btn btn-ghost btn-sm" onClick={applyLink}>
            {href.trim() ? tx("Add link") : tx("Remove link")}
          </button>
        </div>
      )}
      <div role="toolbar" aria-label={tx("Formatting")} className="toolbar">
        <Menu
          label={tx("Text size")}
          placement="above"
          trigger={(props) => (
            <button
              {...props}
              type="button"
              className="tool"
              aria-label={tx("Text size")}
              data-tip={tx("Text size")}
              onMouseDown={(e) => e.preventDefault()}
            >
              <Icon icon={TextFontIcon} size="s" />
              <Icon icon={ArrowDown01Icon} size="xs" />
            </button>
          )}
        >
          {(close) =>
            SIZES.map(([name, size]) => (
              <MenuItem
                key={name}
                onClick={() => {
                  close();
                  if (size) editor.chain().focus().setFontSize(size).run();
                  else editor.chain().focus().unsetFontSize().run();
                }}
              >
                {name}
              </MenuItem>
            ))
          }
        </Menu>
        <span className="tool-sep" />
        <Tool icon={TextBoldIcon} label={tx("Bold")} active={state.bold} onClick={() => editor.chain().focus().toggleBold().run()} />
        <Tool icon={TextItalicIcon} label={tx("Italic")} active={state.italic} onClick={() => editor.chain().focus().toggleItalic().run()} />
        <Tool icon={TextUnderlineIcon} label={tx("Underline")} active={state.underline} onClick={() => editor.chain().focus().toggleUnderline().run()} />
        <Tool icon={TextStrikethroughIcon} label={tx("Strikethrough")} active={state.strike} onClick={() => editor.chain().focus().toggleStrike().run()} />
        <Menu
          label={tx("Text colour")}
          placement="above"
          trigger={(props) => (
            <button
              {...props}
              type="button"
              className="tool"
              aria-label={tx("Text colour")}
              data-tip={tx("Text colour")}
              onMouseDown={(e) => e.preventDefault()}
            >
              <Icon icon={TextColorIcon} size="s" />
            </button>
          )}
        >
          {(close) => (
            <div className="swatches">
              {COLOURS.map((c) => (
                <button
                  key={c}
                  type="button"
                  className="swatch"
                  aria-label={`Colour ${c}`}
                  style={{ background: c }}
                  onClick={() => {
                    close();
                    editor.chain().focus().setColor(c).run();
                  }}
                />
              ))}
            </div>
          )}
        </Menu>
        <span className="tool-sep" />
        <Tool icon={LeftToRightListBulletIcon} label={tx("Bulleted list")} active={state.bullets} onClick={() => editor.chain().focus().toggleBulletList().run()} />
        <Tool icon={LeftToRightListNumberIcon} label={tx("Numbered list")} active={state.numbers} onClick={() => editor.chain().focus().toggleOrderedList().run()} />
        <Tool icon={QuoteDownIcon} label={tx("Quote")} active={state.quote} onClick={() => editor.chain().focus().toggleBlockquote().run()} />
        <span className="tool-sep" />
        <Tool icon={TextAlignLeftIcon} label={tx("Align left")} onClick={() => editor.chain().focus().setTextAlign("left").run()} />
        <Tool icon={TextAlignCenterIcon} label={tx("Align centre")} active={state.center} onClick={() => editor.chain().focus().setTextAlign("center").run()} />
        <Tool icon={TextAlignRightIcon} label={tx("Align right")} active={state.right} onClick={() => editor.chain().focus().setTextAlign("right").run()} />
        <span className="tool-sep" />
        <Tool
          icon={Link01Icon}
          label={tx("Link")}
          active={state.link}
          onClick={() => {
            setHref((editor.getAttributes("link").href as string | undefined) ?? "");
            setLinking((v) => !v);
          }}
        />
        <label className="tool" aria-label={tx("Insert a picture")} data-tip={tx("Insert a picture")}>
          <Icon icon={Image01Icon} size="s" />
          <input
            type="file"
            accept="image/png,image/jpeg,image/gif,image/webp"
            className="hidden"
            onChange={(e) => {
              const file = e.target.files?.[0];
              if (file) onImage(file);
              e.target.value = "";
            }}
          />
        </label>
        <Tool icon={TextClearIcon} label={tx("Clear formatting")} onClick={() => editor.chain().focus().unsetAllMarks().clearNodes().run()} />
      </div>
    </>
  );
}

export function RichEditor({
  initial,
  autoFocus,
  onChange,
  onProblem,
  onDropFiles,
  belowText,
  aboveToolbar,
  placeholder = tx("Write your message"),
  label = "Message",
  spellCheck = true,
  insertRef,
  typeRef,
  replaceRef,
  busy,
  toolbar = true,
}: {
  /** Lets the panel around it type words out where the cursor is, a few at a time. */
  typeRef?: React.RefObject<((text: string) => Promise<void>) | null>;
  /** Said where the writing goes, while something is being written for an empty message. */
  busy?: string;
  /** Formatting tools under the text. Off keeps writing plain and the box calm. */
  toolbar?: boolean;
  /** Lets the panel around it swap in new words for the whole message. */
  replaceRef?: React.RefObject<((html: string) => void) | null>;
  /** Lets the panel around it add words where the cursor is. */
  insertRef?: React.RefObject<((text: string) => void) | null>;
  initial: string;
  autoFocus: boolean;
  onChange: (html: string, text: string) => void;
  onProblem: (message: string) => void;
  /** Files dropped into the text become attachments. */
  onDropFiles: (files: File[]) => void;
  belowText?: React.ReactNode;
  aboveToolbar?: React.ReactNode;
  placeholder?: string;
  label?: string;
  spellCheck?: boolean;
}) {
  const insertImage = async (file: File, editor: Editor) => {
    if (file.size > MAX_INLINE_IMAGE) {
      onProblem(tx("Pictures inside a message can be up to 5 MB. Attach larger ones as files."));
      return;
    }
    const src = await readAsDataUrl(file);
    editor.chain().focus().setImage({ src, alt: file.name }).run();
  };

  const editor = useEditor({
    extensions: [
      StarterKit.configure({
        link: { openOnClick: false, autolink: true, HTMLAttributes: { rel: "noopener noreferrer" } },
      }),
      TextStyleKit.configure({ backgroundColor: false, fontFamily: false, lineHeight: false }),
      Image.configure({ allowBase64: true }),
      Placeholder.configure({ placeholder }),
      TextAlign.configure({ types: ["paragraph", "heading"] }),
    ],
    content: initial,
    autofocus: autoFocus ? "start" : false,
    editorProps: {
      attributes: {
        class: "editor selectable",
        "aria-label": label,
        role: "textbox",
        spellcheck: spellCheck ? "true" : "false",
      },
      handlePaste: (_view, event) => {
        const images = [...(event.clipboardData?.files ?? [])].filter((f) =>
          f.type.startsWith("image/"),
        );
        if (images.length === 0 || !editor) return false;
        images.forEach((f) => void insertImage(f, editor));
        return true;
      },
      handleDrop: (_view, event) => {
        const files = [...(event.dataTransfer?.files ?? [])];
        if (files.length === 0) return false;
        event.preventDefault();
        onDropFiles(files);
        return true;
      },
    },
    onUpdate: ({ editor: e }) => onChange(e.getHTML(), e.getText({ blockSeparator: "\n" })),
  });

  useEffect(() => {
    if (replaceRef) replaceRef.current = (html) => editor?.commands.setContent(html, { emitUpdate: true });
    if (!insertRef) return;
    insertRef.current = (text) => {
      editor?.chain().focus().insertContent(textAsHtml(text)).run();
    };
    if (typeRef) {
      // Like a chat answer: words appear as they come, not all at once. Lines become
      // paragraphs. It stops if the editor goes away, as when the reply is closed.
      // Whatever is due goes in once per frame, as one change, so it runs evenly however
      // long the reply is; a long one takes about a second and a half.
      typeRef.current = async (text) => {
        const parts = text.split(/(\n|[^\S\n]+)/).filter(Boolean);
        const takes = Math.min(1500, parts.length * 30);
        const started = performance.now();
        editor?.commands.focus();
        let shown = 0;
        while (shown < parts.length) {
          await new Promise((next) => requestAnimationFrame(next));
          if (!editor || editor.isDestroyed) return;
          const due = Math.min(parts.length, Math.ceil(((performance.now() - started) / takes) * parts.length));
          let chain = editor.chain();
          for (const part of parts.slice(shown, due)) {
            chain = part === "\n" ? chain.splitBlock() : chain.insertContent({ type: "text", text: part });
          }
          chain.run();
          shown = due;
        }
      };
    }
    return () => {
      insertRef.current = null;
      if (typeRef) typeRef.current = null;
    };
  }, [editor, insertRef, typeRef]);

  if (!editor) return null;
  return (
    <>
      <div className={`composer-body ${busy ? "editor-busy" : ""}`}>
        {busy && (
          <div className="editor-writing" role="status">
            <Spinner size="s" />
            <span>
              {busy}
              <span className="typing-dots" aria-hidden="true">
                <span>.</span>
                <span>.</span>
                <span>.</span>
              </span>
            </span>
          </div>
        )}
        <EditorContent editor={editor} className="grow" />
        {belowText}
      </div>
      {aboveToolbar}
      {toolbar && <Toolbar editor={editor} onImage={(f) => void insertImage(f, editor)} />}
    </>
  );
}
