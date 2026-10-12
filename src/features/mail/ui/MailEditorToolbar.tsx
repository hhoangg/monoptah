import { useEditorState, type Editor } from "@tiptap/react";
import { useState, type ComponentType } from "react";
import {
  Bold,
  Code,
  ImagePlus,
  Italic,
  Link,
  ListBullet,
  ListOrdered,
  Quote,
  Strikethrough,
  Underline,
} from "../../../shared/ui/icons";

type Props = {
  editor: Editor | null;
  /** Opens the picker; the composer owns files, the toolbar owns no state for them. */
  onImage: () => void;
  linkOpen: boolean;
  onLinkOpenChange: (open: boolean) => void;
};

const BTN =
  "grid h-[22px] w-6 place-items-center rounded-[5px] text-content/45 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-40 disabled:hover:bg-transparent";
const ON = "bg-content/10 text-content";

type Item = {
  label: string;
  title: string;
  icon: ComponentType<{ className?: string; strokeWidth?: number }>;
  mark: string;
  run: (editor: Editor) => void;
};

const GROUPS: Item[][] = [
  [
    {
      label: "Bold",
      title: "Bold  \u2318B",
      icon: Bold,
      mark: "bold",
      run: (e) => e.chain().focus().toggleBold().run(),
    },
    {
      label: "Italic",
      title: "Italic  \u2318I",
      icon: Italic,
      mark: "italic",
      run: (e) => e.chain().focus().toggleItalic().run(),
    },
    {
      label: "Underline",
      title: "Underline  \u2318U",
      icon: Underline,
      mark: "underline",
      run: (e) => e.chain().focus().toggleUnderline().run(),
    },
    {
      label: "Strikethrough",
      title: "Strikethrough",
      icon: Strikethrough,
      mark: "strike",
      run: (e) => e.chain().focus().toggleStrike().run(),
    },
  ],
  [
    {
      label: "Bullet list",
      title: "Bullet list",
      icon: ListBullet,
      mark: "bulletList",
      run: (e) => e.chain().focus().toggleBulletList().run(),
    },
    {
      label: "Numbered list",
      title: "Numbered list",
      icon: ListOrdered,
      mark: "orderedList",
      run: (e) => e.chain().focus().toggleOrderedList().run(),
    },
    {
      label: "Blockquote",
      title: "Blockquote",
      icon: Quote,
      mark: "blockquote",
      run: (e) => e.chain().focus().toggleBlockquote().run(),
    },
    {
      label: "Code",
      title: "Code",
      icon: Code,
      mark: "code",
      run: (e) => e.chain().focus().toggleCode().run(),
    },
  ],
];

const Sep = () => <i className="mx-[3px] h-3.5 w-px shrink-0 bg-stroke" />;

/** The mockup's format bar. Icons only, so every button is named for assistive tech. */
export function MailEditorToolbar({
  editor,
  onImage,
  linkOpen,
  onLinkOpenChange,
}: Props) {
  const state = useEditorState({
    editor,
    selector: ({ editor: e }) => ({
      marks: GROUPS.flat()
        .filter((item) => e?.isActive(item.mark))
        .map((item) => item.mark),
      link: e?.isActive("link") ?? false,
      href: (e?.getAttributes("link").href as string | undefined) ?? "",
    }),
    equalityFn: (a, b) =>
      !!a &&
      !!b &&
      a.link === b.link &&
      a.href === b.href &&
      a.marks.join() === b.marks.join(),
  });
  const marks = state?.marks ?? [];
  const link = state?.link ?? false;
  const href = state?.href ?? "";
  const [draft, setDraft] = useState("");

  const apply = () => {
    if (!editor) return;
    const raw = draft.trim();
    const chain = editor.chain().focus().extendMarkRange("link");
    if (raw === "") chain.unsetLink().run();
    else {
      // A bare "example.com" would otherwise be saved as a relative link.
      const href = /^[a-z][a-z0-9+.-]*:/i.test(raw) ? raw : `https://${raw}`;
      chain.setLink({ href }).run();
    }
    onLinkOpenChange(false);
  };

  return (
    <div className="border-b border-stroke">
      <div
        role="toolbar"
        aria-label="Formatting"
        className="flex items-center gap-[3px] px-2 py-1.5"
      >
        {GROUPS.map((group, i) => (
          <span key={i} className="flex items-center gap-[3px]">
            {i > 0 ? <Sep /> : null}
            {group.map((item) => (
              <button
                key={item.label}
                type="button"
                aria-label={item.label}
                title={item.title}
                aria-pressed={marks.includes(item.mark)}
                disabled={!editor}
                // Keep the selection: a click must not move focus out of the editor.
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => editor && item.run(editor)}
                className={`${BTN} ${marks.includes(item.mark) ? ON : ""}`}
              >
                <item.icon className="size-3.5" strokeWidth={1.75} />
              </button>
            ))}
          </span>
        ))}
        <Sep />
        <button
          type="button"
          aria-label="Link"
          title={"Link  \u2318K"}
          aria-pressed={link}
          disabled={!editor}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => {
            setDraft(href);
            onLinkOpenChange(!linkOpen);
          }}
          className={`${BTN} ${link ? ON : ""}`}
        >
          <Link className="size-3.5" strokeWidth={1.75} />
        </button>
        <button
          type="button"
          aria-label="Inline image"
          title="Inline image"
          disabled={!editor}
          onMouseDown={(event) => event.preventDefault()}
          onClick={onImage}
          className={BTN}
        >
          <ImagePlus className="size-3.5" strokeWidth={1.75} />
        </button>
      </div>
      {linkOpen ? (
        <form
          className="flex items-center gap-2 border-t border-stroke px-2 py-1.5"
          onSubmit={(event) => {
            event.preventDefault();
            apply();
          }}
        >
          <input
            autoFocus
            aria-label="Link address"
            value={draft}
            placeholder="https://example.com (empty removes the link)"
            onChange={(event) => setDraft(event.target.value)}
            className="h-7 min-w-0 flex-1 rounded-md border border-content/10 bg-transparent px-2 text-[12px] text-content outline-none placeholder:text-content/35 focus:border-accent/50"
          />
          <button
            type="submit"
            className="rounded-md border border-content/10 px-2.5 py-1 text-[12px] text-content/70 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
          >
            Apply
          </button>
        </form>
      ) : null}
    </div>
  );
}
