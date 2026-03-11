# PRfait

**/pɹ̩.feɪ/** — a quadruple pun across two languages. It's **PR fait** (French: "PR done"), **parfait** (French: "perfect", English: the dessert), and sounds like **PR fate** — because every pull request deserves to meet its destiny, not languish in review limbo.

A terminal UI for reviewing pull requests with semantic diff analysis.

PRfait fetches open PRs from configured GitHub repositories, runs semantic analysis (via [inspect-core](https://github.com/Ataraxy-Labs/inspect) and [sem](https://github.com/Ataraxy-Labs/sem)) to identify meaningful entity-level changes, and presents them in a TUI with structural diffs, risk scoring, and inline review comments.

## Features

- **Semantic diffs** — diffs are grouped by entity (function, type, module) rather than raw line changes, powered by tree-sitter
- **Risk scoring** — entities are ranked by risk score incorporating blast radius, dependency count, public API surface, and change classification
- **Cross-PR overlap detection** — when the same entity is modified in multiple open PRs, it's flagged as a merge conflict risk and boosted in sort order
- **Inline review comments** — write review comments directly in the TUI, with multi-line drag selection and GitHub suggestion blocks
- **Side-by-side and unified diff modes** — toggle with `d`
- **External editor integration** — press `e` to open a file in your editor; edits are converted to GitHub suggestion comments automatically
- **Reviewed file tracking** — mark files as reviewed with `x`; state persists across sessions

## Setup

Create a config file at `~/.config/prfait/config.toml`:

```toml
github_token = "ghp_..."  # or omit to use `gh auth token`

[[repos]]
name = "owner/repo"
local_path = "/path/to/local/clone"  # optional, enables structural diffs
```

## Usage

```
cargo run
```

### Key bindings

| Key | Action |
|-----|--------|
| `j`/`k` or arrows | Navigate |
| `Tab` | Toggle focus between PR list and diff view |
| `Enter`/`c` | Open inline comment editor |
| `Alt+Enter` | Save comment |
| `Esc` | Cancel / quit |
| `d` | Toggle unified / side-by-side diff |
| `e` | Open file in external editor |
| `x` | Toggle file reviewed |
| `Ctrl+R` | Submit review |
| `r` | Refresh PR list |
| `g`/`G` | Scroll to top / bottom |

## Building

```
cargo build --release
```

The binary will be at `target/release/prfait`.

## License

See repository for license details.
