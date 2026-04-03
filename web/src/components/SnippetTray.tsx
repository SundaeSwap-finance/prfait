import type { Snippet } from "../hooks/useStory";

interface SnippetTrayProps {
  snippets: Snippet[];
  onRemove: (id: string) => void;
}

const TYPE_LABELS: Record<Snippet["type"], string> = {
  hunk: "Hunk",
  lines: "Lines",
  text: "Text",
};

export default function SnippetTray({ snippets, onRemove }: SnippetTrayProps) {
  return (
    <div className="snippet-tray">
      <div className="snippet-tray-header">
        Captured ({snippets.length})
      </div>
      <div className="snippet-tray-list">
        {snippets.map((s) => (
          <div key={s.id} className="snippet-card">
            <div className="snippet-card-header">
              <span className={`snippet-type snippet-type-${s.type}`}>
                {TYPE_LABELS[s.type]}
              </span>
              <span className="snippet-file">{s.file.split("/").pop()}</span>
              {s.startLine > 0 && (
                <span className="snippet-lines">
                  L{s.startLine}
                  {s.endLine !== s.startLine ? `–${s.endLine}` : ""}
                </span>
              )}
              <button
                className="snippet-remove"
                onClick={() => onRemove(s.id)}
                title="Remove"
              >
                ×
              </button>
            </div>
            <pre className="snippet-preview">{s.content.slice(0, 200)}</pre>
          </div>
        ))}
      </div>
    </div>
  );
}
