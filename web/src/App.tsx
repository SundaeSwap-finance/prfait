import { useState } from "react";
import {
  useMeta,
  useChangedFiles,
  useCommits,
  useFileDiff,
  useCommitDiff,
} from "./hooks/useApi";
import FileTree from "./components/FileTree";
import DiffViewer from "./components/DiffViewer";

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
        />
      </div>
    </div>
  );
}
