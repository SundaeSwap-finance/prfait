import { useState, useCallback } from "react";
import {
  useMeta,
  useChangedFiles,
  useCommits,
  useFileDiff,
  useCommitDiff,
} from "./hooks/useApi";
import { useStory } from "./hooks/useStory";
import FileTree from "./components/FileTree";
import DiffViewer from "./components/DiffViewer";
import StoryEditor from "./components/StoryEditor";

interface CaptureMode {
  insertAtIndex: number;
  filterFile?: string;
  /** If the capture was triggered from a narration panel, its ID — remove if still empty on capture */
  sourceNarrationId?: string;
}

export default function App() {
  const meta = useMeta();
  const { files, loading: filesLoading } = useChangedFiles();
  const commits = useCommits();
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [selectedCommit, setSelectedCommit] = useState<string | null>(null);
  const [view, setView] = useState<"story" | "diff">("story");
  const [captureMode, setCaptureMode] = useState<CaptureMode | null>(null);

  const { diff: fileDiff, loading: fileDiffLoading } =
    useFileDiff(selectedCommit ? null : selectedFile);
  const { diff: commitDiff, loading: commitDiffLoading } =
    useCommitDiff(selectedCommit);

  const activeDiff = selectedCommit ? commitDiff : fileDiff;
  const diffLoading = selectedCommit ? commitDiffLoading : fileDiffLoading;

  const {
    summary,
    setSummary,
    panels,
    addSnippet,
    addSnippetAt,
    addNarration,
    updateNarration,
    updateSnippet,
    setSnippetComment,
    removePanel,
    reorderPanels,
    loaded,
  } = useStory();

  const handleRequestCapture = useCallback(
    (insertAtIndex: number, filterFile?: string, sourceNarrationId?: string) => {
      setCaptureMode({ insertAtIndex, filterFile, sourceNarrationId });
      if (filterFile) {
        setSelectedFile(filterFile);
        setSelectedCommit(null);
      }
      setView("diff");
    },
    []
  );

  const handleAddSnippet = useCallback(
    (snippet: Parameters<typeof addSnippet>[0]) => {
      if (captureMode) {
        // If the source narration is still empty, remove it and adjust index
        let insertIdx = captureMode.insertAtIndex;
        if (captureMode.sourceNarrationId) {
          const srcPanel = panels.find((p) => p.id === captureMode.sourceNarrationId);
          if (srcPanel && srcPanel.kind === "narration" && srcPanel.markdown === "") {
            const srcIdx = panels.indexOf(srcPanel);
            removePanel(srcPanel.id);
            // Adjust insert index since we removed a panel before it
            if (srcIdx < insertIdx) insertIdx--;
          }
        }
        addSnippetAt(snippet, insertIdx);
        setCaptureMode(null);
        setView("story");
      } else {
        addSnippet(snippet);
      }
    },
    [captureMode, panels, addSnippet, addSnippetAt, removePanel]
  );

  const handleCancelCapture = useCallback(() => {
    setCaptureMode(null);
    setView("story");
  }, []);

  return (
    <div className="app">
      <header className="header">
        <h1>prfait</h1>
        {meta && (
          <>
            <span className="branch">{meta.branch}</span>
            <span className="base">vs {meta.base}</span>
          </>
        )}

        <div className="view-toggle">
          <button
            className={`view-toggle-btn ${view === "story" ? "active" : ""}`}
            onClick={() => {
              setView("story");
              if (captureMode) setCaptureMode(null);
            }}
          >
            Story
          </button>
          <button
            className={`view-toggle-btn ${view === "diff" ? "active" : ""}`}
            onClick={() => setView("diff")}
          >
            Diff
          </button>
        </div>

        {panels.length > 0 && (
          <span className="panel-count">
            {panels.length} panel{panels.length !== 1 ? "s" : ""}
          </span>
        )}
      </header>

      {captureMode && view === "diff" && (
        <div className="capture-banner">
          <span>Select a snippet to insert into your story</span>
          <button onClick={handleCancelCapture}>Cancel</button>
        </div>
      )}

      {view === "story" ? (
        <div className="main story-view">
          <StoryEditor
            summary={summary}
            onUpdateSummary={setSummary}
            panels={panels}
            files={files}
            onAddNarration={addNarration}
            onUpdateNarration={updateNarration}
            onUpdateSnippet={updateSnippet}
            onSetSnippetComment={setSnippetComment}
            onRemovePanel={removePanel}
            onReorderPanels={reorderPanels}
            onRequestCapture={handleRequestCapture}
          />
        </div>
      ) : (
        <div className="main diff-view">
          {filesLoading ? (
            <div className="sidebar">
              <div className="loading">Loading</div>
            </div>
          ) : (
            <FileTree
              files={files}
              commits={commits}
              selectedFile={selectedFile}
              selectedCommit={selectedCommit}
              onSelectFile={setSelectedFile}
              onSelectCommit={(hash) => {
                setSelectedCommit(hash === selectedCommit ? null : hash);
              }}
            />
          )}

          <DiffViewer
            diff={activeDiff}
            loading={diffLoading}
            filePath={selectedFile}
            commitHash={selectedCommit}
            onAddSnippet={handleAddSnippet}
          />
        </div>
      )}
    </div>
  );
}
