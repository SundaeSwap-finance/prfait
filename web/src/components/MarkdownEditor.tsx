import { useEditor, EditorContent } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import Placeholder from "@tiptap/extension-placeholder";
import { Markdown } from "tiptap-markdown";
import { useEffect, useRef } from "react";

interface MarkdownEditorProps {
  value: string;
  onChange: (markdown: string) => void;
  onKeyDown?: (e: React.KeyboardEvent) => void;
  placeholder?: string;
  autoFocus?: boolean;
}

export default function MarkdownEditor({
  value,
  onChange,
  onKeyDown,
  placeholder = "Write narration...",
  autoFocus = false,
}: MarkdownEditorProps) {
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  const editor = useEditor({
    extensions: [
      StarterKit.configure({
        heading: { levels: [1, 2, 3] },
      }),
      Placeholder.configure({ placeholder }),
      Markdown,
    ],
    content: value,
    onUpdate({ editor }) {
      const md = (editor.storage.markdown as any).getMarkdown() as string;
      onChangeRef.current(md);
    },
    editorProps: {
      attributes: {
        class: "narration-editor-content",
      },
    },
  });

  // Sync external value changes (e.g. when @ command strips text)
  const lastExternalValue = useRef(value);
  useEffect(() => {
    if (!editor) return;
    if (value !== lastExternalValue.current) {
      lastExternalValue.current = value;
      const currentMd = (editor.storage.markdown as any).getMarkdown() as string;
      if (currentMd !== value) {
        editor.commands.setContent(value);
      }
    }
  }, [value, editor]);

  // Track internal changes so we don't fight with the external sync
  useEffect(() => {
    if (!editor) return;
    const handler = () => {
      lastExternalValue.current = (editor.storage.markdown as any).getMarkdown() as string;
    };
    editor.on("update", handler);
    return () => { editor.off("update", handler); };
  }, [editor]);

  useEffect(() => {
    if (autoFocus && editor) {
      editor.commands.focus("end");
    }
  }, [autoFocus, editor]);

  return (
    <div className="markdown-editor" onKeyDown={onKeyDown}>
      <EditorContent editor={editor} />
    </div>
  );
}
