import { Plugin, PluginKey } from "@tiptap/pm/state";
import Image from "@tiptap/extension-image";
import StarterKit from "@tiptap/starter-kit";
import {
  EditorContent,
  Extension,
  useEditor,
  type Editor,
} from "@tiptap/react";
import { useEffect, useRef, useState } from "react";
import type { TipTapNode } from "../model/mailCompose";
import { MailEditorToolbar } from "./MailEditorToolbar";

export type MailEditorProps = {
  /**
   * A saved draft's TipTap JSON, or null for an empty body. The type is the
   * guard: TipTap holds what the person typed and never a received message,
   * whose HTML it would rewrite to fit its schema (spec 7.5).
   */
  content: TipTapNode | null;
  onChange: (doc: TipTapNode) => void;
  onSend: () => void;
  /** Files pasted or dropped into the body; the composer stages them. */
  onFiles: (files: File[]) => void;
  /** Toolbar image button. */
  onImage: () => void;
  onEditor: (editor: Editor | null) => void;
};

type Handlers = Pick<MailEditorProps, "onSend" | "onFiles"> & {
  onLink: () => void;
};

/** Lines to inline nodes, so the text is never parsed as HTML. */
const plainContent = (text: string) =>
  text
    .split(/\r\n?|\n/)
    .flatMap((line, i) => [
      ...(i > 0 ? [{ type: "hardBreak" }] : []),
      ...(line ? [{ type: "text", text: line }] : []),
    ]);

function pastePlain(editor: Editor) {
  navigator.clipboard
    .readText()
    .then((text) => {
      if (text) editor.chain().focus().insertContent(plainContent(text)).run();
    })
    // The webview can refuse clipboard reads; the keystroke then does nothing.
    .catch(() => undefined);
}

/**
 * The parts Tiptap Pro would sell and spec 3 says to write: files pasted or
 * dropped (handled by the composer, not inserted blindly), paste as plain text
 * and send on Cmd+Enter. Handlers sit behind a ref so the extension, which
 * must be created once, always calls the current closures.
 */
const MailKeys = Extension.create<{ handlers: { current: Handlers } }>({
  name: "mailCompose",
  addKeyboardShortcuts() {
    return {
      "Mod-Enter": () => {
        this.options.handlers.current.onSend();
        return true;
      },
      "Mod-k": () => {
        this.options.handlers.current.onLink();
        return true;
      },
      "Mod-Shift-v": () => {
        pastePlain(this.editor);
        return true;
      },
    };
  },
  addProseMirrorPlugins() {
    const { handlers } = this.options;
    return [
      new Plugin({
        key: new PluginKey("mailFiles"),
        props: {
          handlePaste: (_view, event) => {
            const files = [...(event.clipboardData?.files ?? [])];
            if (files.length === 0) return false;
            handlers.current.onFiles(files);
            return true;
          },
          handleDrop: (_view, event, _slice, moved) => {
            // A drag inside the document is ProseMirror's own move.
            if (moved) return false;
            const files = [...(event.dataTransfer?.files ?? [])];
            if (files.length === 0) return false;
            event.preventDefault();
            handlers.current.onFiles(files);
            return true;
          },
        },
      }),
    ];
  },
});

/**
 * The serializer reads `data-mono-att` and nothing else, so a mistake here
 * must fail loudly at send, not half work.
 */
const MailImage = Image.extend({
  addAttributes() {
    return {
      ...this.parent?.(),
      "data-mono-att": {
        default: null,
        parseHTML: (el: HTMLElement) => el.getAttribute("data-mono-att"),
        renderHTML: (attrs: Record<string, unknown>) =>
          attrs["data-mono-att"]
            ? { "data-mono-att": String(attrs["data-mono-att"]) }
            : {},
      },
    };
  },
});

const EDITOR_CLASS =
  "min-h-[180px] px-3 py-2.5 text-[13px] leading-relaxed text-content " +
  "outline-none [&_p]:my-0 [&_p+p]:mt-3 " +
  "[&_a]:text-accent [&_a]:underline " +
  "[&_blockquote]:border-l-2 [&_blockquote]:border-content/20 [&_blockquote]:pl-3 " +
  "[&_code]:rounded [&_code]:bg-content/8 [&_code]:px-1 [&_code]:py-0.5 " +
  "[&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5 " +
  "[&_pre]:rounded [&_pre]:bg-content/8 [&_pre]:p-2 " +
  "[&_img]:h-auto [&_img]:max-w-full";

export default function MailEditor({
  content,
  onChange,
  onSend,
  onFiles,
  onImage,
  onEditor,
}: MailEditorProps) {
  const [linkOpen, setLinkOpen] = useState(false);
  const handlers = useRef<Handlers>({ onSend, onFiles, onLink: () => {} });
  handlers.current = {
    onSend,
    onFiles,
    onLink: () => setLinkOpen((open) => !open),
  };
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  const editor = useEditor({
    extensions: [
      StarterKit.configure({
        heading: { levels: [1, 2, 3] },
        link: {
          openOnClick: false,
          autolink: true,
          protocols: ["http", "https", "mailto"],
        },
      }),
      MailImage.configure({ inline: false, allowBase64: false }),
      MailKeys.configure({ handlers }),
    ],
    content: content ?? undefined,
    editorProps: {
      attributes: { class: EDITOR_CLASS, "aria-label": "Message body" },
    },
    onUpdate: ({ editor: e }) => onChangeRef.current(e.getJSON() as TipTapNode),
  });

  useEffect(() => {
    onEditor(editor);
    return () => onEditor(null);
  }, [editor, onEditor]);

  return (
    <div className="mt-3 overflow-hidden rounded-lg border border-content/10">
      <MailEditorToolbar
        editor={editor}
        onImage={onImage}
        linkOpen={linkOpen}
        onLinkOpenChange={setLinkOpen}
      />
      <EditorContent editor={editor} />
    </div>
  );
}
