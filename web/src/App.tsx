import { useState } from "react";
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
import SnippetTray from "./components/SnippetTray";

export default function App() {
  const meta = useMeta();
  const { files, loading: filesLoading } = useChangedFiles();
  const commits = useCommits();
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [selectedCommit, setSelectedCommit] = useState<string | null>(null);

  const { diff: fileDiff, loading: fileDiffLoading } =
    useFileDiff(selectedCommit ? null : selectedFile);
  const { diff: commitDiff, loading: commitDiffLoading } =
    useCommitDiff(selectedCommit);

  const activeDiff = selectedCommit ? commitDiff : fileDiff;
  const diffLoading = selectedCommit ? commitDiffLoading : fileDiffLoading;

  const { snippets, addSnippet, removeSnippet } = useStory();

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
        {snippets.length > 0 && (
          <span className="snippet-count">
            {snippets.length} snippet{snippets.length !== 1 ? "s" : ""}
          </span>
        )}
      </header>

      <div className="main">
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
          onAddSnippet={addSnippet}
        />

        {snippets.length > 0 && (
          <SnippetTray snippets={snippets} onRemove={removeSnippet} />
        )}
      </div>
    </div>
  );
}
