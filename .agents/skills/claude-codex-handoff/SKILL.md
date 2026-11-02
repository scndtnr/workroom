---
name: claude-codex-handoff
description: このリポジトリで Claude Code と Codex の作業を分ける。Claude はユーザーとのやり取り・計画・レビューを、Codex は合意した計画の実装を担当する。Codex に実装を渡すとき、Codex の成果をレビューするとき、Codex に追加のレビューを頼むときに使う。
---

# claude-codex-handoff

## 役割

| | Claude Code | Codex |
| --- | --- | --- |
| ユーザーとのやり取り | 担当：依頼の確認、質問、結果の報告 | — |
| 計画 | 書いて、ユーザーと合意する | 計画がコードに合わなければ指摘する |
| 実装 | 小さな修正のみ | 担当：コード、テスト、確認のコマンド |
| レビュー | Codex の差分はコミット前に必ずレビューする | 任意。ファイルを書き換える処理など、危険な変更のときに追加でレビューする |
| コミット | 担当 | — |

## 流れ

1. Claude が短い計画をユーザーと合意する：目的、対象のファイル、対象外、完了の条件。
2. Claude が下の「引き継ぎ書」をファイルに書き、Codex に実装させる。
3. Claude が差分と確認の出力をレビューする。問題があれば、指摘を同じ Codex のセッションに `resume` で送る。
4. 任意：ファイルの書き換え・削除など危険な変更では、Codex に読み取り専用のレビューを頼む。
5. Claude がコミットし、ユーザーに報告する（push・リポジトリの作成はユーザーの確認を取ってから）。

## 引き継ぎ書

```md
# Task: <短い題名>

Goal:
Scope (files):
Out of scope:
Decisions already made:
Done when:
- cargo fmt --check
- cargo clippy --all-targets -- -D warnings
- cargo test
Report back: what changed, verification output, open risks.
Follow AGENTS.md. Do not commit.
```

## 呼び出し方（PowerShell）

プロンプトはファイルで渡す。PowerShell でプロンプトを直接書くと、`codex exec` が標準入力を待って止まる。
プロンプトのファイルと出力の jsonl は、リポジトリの外（一時ディレクトリなど）に置き、使い終わったら消す。コミットしない。

新しい Codex のセッションは、必ず `Requested-By: claude-code <セッション ID>` の行で始める。ID は `$env:CLAUDE_CODE_SESSION_ID` からコマンドで入れる（手で書かない）。journalkit の `digest` が、この行で Codex のセッションを依頼元の Claude Code のセッションに結び付ける。引き継ぎ書は `# Task: <短い題名>` で始める（digest が題名に使う）。

実装：

```powershell
& { "Requested-By: claude-code $env:CLAUDE_CODE_SESSION_ID"; Get-Content $task -Encoding utf8 } | codex exec --json -m gpt-6.1-sol -c 'model_reasoning_effort="xhigh"' -s workspace-write - > $implJsonl
```

レビューの指摘を同じセッションに送る。`--json` の出力のうち、`thread.started` イベントの `thread_id` を使う（`--last` は直近のセッションを再開するので、間にレビューを挟んでいると別のセッションになる）。`exec resume` には `-s` がないので、サンドボックスは設定で渡す（通らなければ `codex exec resume --help` で確かめる）：

```powershell
Get-Content $fix -Encoding utf8 | codex exec resume <thread_id> --json -m gpt-6.1-sol -c 'model_reasoning_effort="xhigh"' -c 'sandbox_mode="workspace-write"' - > $fixJsonl
```

任意の読み取り専用レビュー：

```powershell
& { "Requested-By: claude-code $env:CLAUDE_CODE_SESSION_ID"; Get-Content $reviewPrompt -Encoding utf8 } | codex exec --json -m gpt-6.1-sol -c 'model_reasoning_effort="xhigh"' -s read-only - > $reviewJsonl
```

Codex の最終報告は、`item.type` が `agent_message` である最後の `item.completed` イベントから読む。

- 既定は `gpt-6.1-sol`、`xhigh`。権限・安全性の境界に関わる変更では `gpt-6-astra` も検討する。
- モデル名が通らない・古そうなときは、`~/.codex/models_cache.json` で現在の一覧を確かめ、このスキルを直す。
- `workspace-write` ではネットワークが使えないことがある。依存関係は事前に取得しておく（必要なら `cargo fetch` してから渡す）。
- Codex の Windows サンドボックスは、クラウドの仮想ドライブ（ACL 非対応）では動かない。リポジトリは NTFS 上に置く。
