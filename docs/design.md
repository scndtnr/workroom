# workroom 設計

AI エージェント（Claude Code / Codex など）に、日付付きの「作業部屋」ディレクトリを用意する CLI。
Git 管理するほどでもない調査・作業の場所を、すぐに作れて、すぐに戻れて、あとから探せる状態にする。

- 実行ファイル名は `workroom`。普段打つ関数名は `workroom init <shell> --cmd wk` で `wk` などに変えられる（zoxide の `--cmd` と同じ方式）。
- 参考にしたツール：[tobi/try](https://github.com/tobi/try-cli)、[try-rs](https://github.com/tassiovirginio/try-rs)（UI）、[scry](https://github.com/stephendolan/scry)（エージェント向けという発想）。
  - try 系との違いは、エージェント向けのルール管理、README の1行目を一覧に表示すること、完了マーカー、中身の横断検索、Claude の会話履歴を引き継ぐ import、案件階層。
  - try-rs にある、テーマの切り替え・git clone・worktree は持たない。

## ディレクトリ構成

```
<root>/
├── _workroom/
│   └── AGENTS.md              ルールのテンプレート（部屋を作るときにコピーする）
├── 2026-11-02-fix-bug/         部屋（YYYY-MM-DD-<slug>）
│   ├── README.md               1行目 = 一覧に表示する説明
│   ├── AGENTS.md               ルール（テンプレートのコピー）
│   ├── log.md / docs/ / artifacts/ / tmp/   （エージェントがルールに従って作る）
│   └── .done                   完了マーカー（任意）
└── acme/                       案件ディレクトリ（任意）
    ├── _workroom/AGENTS.md     案件ごとの追加ルール（任意）
    └── 2026-11-02-report/
```

- 部屋の名前は `YYYY-MM-DD-<slug>` とし、slug は `^[a-z0-9][a-z0-9-]*$`。日本語の説明は README.md の1行目に書く。
  - Claude Code は会話履歴をパス由来の名前（英数字以外を `-` に置換）で保存する。日本語を名前に使うと、別の部屋の履歴とぶつかることがあるため。
- 配置は `<root>/<部屋>` と `<root>/<案件>/<部屋>` の両方を自動で判別する。`.` や `_` で始まるディレクトリは案件とみなさない。
  - Google Drive for desktop でフォルダを同期すると、ルート直下に `.tmp.drivedownload` / `.tmp.driveupload` が作られる。これらを案件として扱わないことをテストで固定している。
- **Windows で Codex を使う場合、作業ルートは NTFS 上に置く。** Codex の Windows サンドボックスは、起動時に作業ディレクトリへ ACL（サンドボックス用の書き込み権限）を付ける。Google Drive の仮想ドライブ（`G:` など）は ACL に対応していないため `SetSecurityInfo failed: 87` で失敗し、シェルが一切起動しない（`helper_unknown_error: setup refresh had errors`）。クラウドに残したい場合は、NTFS 上のフォルダを Google Drive for desktop などのフォルダ同期でバックアップする。
- 部屋は移動しない。Claude や Codex の会話履歴は絶対パスに紐づくので、移動すると resume できなくなるため。完了は `.done` で表す。

## エージェントのルール（AGENTS.md）

**ルールの全文を各部屋の AGENTS.md にコピーする。**

- Codex は、Git 管理外のディレクトリでは親の AGENTS.md を自動では読まない。`../AGENTS.md` を読むよう案内する方式だと、エージェントが案内に従ってファイルを読めるかどうかに頼ることになる。部屋の AGENTS.md に全文があれば、それだけでルールが届く。
  - 以前、案内方式で「Codex が `../AGENTS.md` を読めなかった」例があった。これは、読み取りの制限ではなく、Codex のシェル自体が起動していなかったことが原因の可能性が高い（下記の Windows の注意）。
- Claude Code は親ディレクトリの AGENTS.md も読み込む。テンプレートを `<root>/AGENTS.md` に置くと、部屋のコピーと二重に読み込まれる。そこで、自動では読まれない `_workroom/AGENTS.md` に置く。
- コピーした内容の1行目に、生成元の識別子を入れる（`<!-- workroom:rules sha256=<本文の sha256>（…） -->`）。本文は BOM なし・改行 LF にそろえる。
  - `workroom rules sync` を実行すると、生成後に手で変更されていない部屋（本文の sha256 が識別子と一致するもの）だけを、最新のテンプレートで更新する。
  - 以前の PowerShell 版（`wk sync-rules`）が作った、`<!-- wk:` で始まる AGENTS.md も「生成されたもの」とみなし、置き換えの対象にする（ハッシュがないので、手で変更されたかは判別しない）。
- 作業ルート直下に AGENTS.md があると、Claude Code が部屋のコピーと二重に読み込む。自動では移動せず、部屋を作るたびに警告する。
- 案件の部屋には、全体のテンプレートの後ろに、案件のテンプレート（あれば）を連結してコピーする。
- テンプレートの場所は設定 `agent_rules` で変えられる。ファイルがなければ、内蔵の文面（`templates/AGENTS.md`）から作る。

## シェル連携

実行ファイルは、呼び出し元シェルのカレントディレクトリを変えられない。そこで、ラッパー関数を介して移動する。

1. ラッパーが一時ファイルを作り、`workroom --cwd-file <tmp> <引数...>` を呼ぶ。
2. workroom は、1行目に移動先、2行目に（あれば）移動後に起動するコマンド（`claude` / `codex` のみ）を UTF-8・BOM なしで書き出す。
3. ラッパーがそのファイルを読み、cd してから、必要ならコマンドを起動する。

- TUI の描画と標準出力の取り込みがぶつからず、PowerShell / nushell / bash / zsh で同じ方式で動く（yazi の `--cwd-file` と同じ考え方）。
- `claude` / `codex` を実行ファイル側ではなくシェル側で起動するのは、Windows の npm 製の起動スクリプト（`codex.ps1` / `codex.cmd`）を、シェルの通常の解決順で見つけるため。
- ラッパー関数（`init` の出力）は ASCII だけで書く。PowerShell は外部コマンドの出力を `[Console]::OutputEncoding` で読むため、日本語版 Windows の既定（shift_jis）では UTF-8 の日本語が化け、`Invoke-Expression` が構文エラーになる（`-NoProfile` で確認済み）。
- PowerShell のラッパーは、実行ファイルを `Get-Command -CommandType Application` で探して呼ぶ（関数名を `workroom` にしても自分自身を呼ばないように）。既定の関数名は `wk`。
- nushell 0.100 では、外部コマンドが 0 以外で終わるとエラーになり関数が止まる。一時ファイルを消すため、呼び出しを `try` で囲む。
- nushell の `source` には、解析時点で決まっているパスが必要になる。そのため、`workroom init nu --cmd wk | save -f <file>` で一度ファイルに保存し、config.nu から `source <file>` する（zoxide と同じ方式）。

## コマンド

| コマンド | 動作 |
|---|---|
| `wk` / `wk <検索語>` | TUI ピッカー（検索語を入れた状態で起動） |
| `wk new <slug> [説明] [-p 案件] [--date D] [-c\|-x]` | 作成して移動する。既にあれば移動のみ。`-c` で claude、`-x` で codex を起動 |
| `wk ls [検索語] [-a] [--days N] [--depth N]` | tree 表示。既定は直近 N 日で進行中のもののみ |
| `wk grep <pattern> [rg のオプション...]` | 中身の横断検索。rg があれば rg、なければ内蔵の簡易検索 |
| `wk done` / `wk undone [名前]` | 完了マーカーを付ける・外す（名前を省くとカレントの部屋） |
| `wk open [名前]` / `wk root` | ファイラーで開く / ルートへ移動 |
| `wk import <path> [slug] [説明] [--date D] [-p 案件]` | 既存のディレクトリを移行する（下記） |
| `wk rules sync` | 各部屋の AGENTS.md をテンプレートに合わせて更新する |
| `workroom init <pwsh\|nu\|bash\|zsh> [--cmd 名前]` | ラッパー関数を出力する |
| `workroom config [--init]` | 設定ファイルのパスと内容を表示する（`--init` で雛形を作成） |

- サブコマンド名と一致しない語は検索語になる。サブコマンドと同じ語を検索するときは `wk -- done`。PowerShell は素の `--` を自分で消費するので `wk '--' done` と書く。

## TUI ピッカー（try-rs を参考に）

```
┌ wk  ~/workroom ──────────────────────────────┐┌ プレビュー ──────────────────┐
│> b                          1/3  最近使った順││# 不具合調査                  │
│> 26-11-02 fix-bug  不具合調査                ││- created: 2026-11-02 19:33   │
│  + 新規作成: b                               ││docs/                         │
│                                              ││  調査まとめ.md               │
│                                              ││3 files · 12.4 KB             │
└──────────────────────────────────────────────┘└──────────────────────────────┘
Enter 移動  ^D 完了  ^O 開く  ^S 並び順  Tab 完了済み  Alt+P 詳細  Esc 終了
```

- `wk` で開く。`wk <検索語>` なら、検索語を入力した状態で開く。標準入力・標準出力が端末でなければ、案内を出して終了コード 2 で終わる。
- 新規作成は「+ 新規作成」の行で `Enter`（専用のキーは設けない。`Ctrl+N` は fzf と同じく選択の移動）。検索語が slug として正しくなければ、小文字にして空白・`_` を `-` に置き換えたものを使う。それでも正しくなければ作らずに案内を出す。
- 一覧の各行：アイコン（任意）、日付（`YY-MM-DD`）、`案件/`slug、README の1行目、完了マーク。
- 完了済みの部屋は、検索語があっても `Tab` を押すまで隠す（隠している件数を表示する）。`wk ls <検索語>` は完了済みも表示するので、扱いが異なる。
- 画面を表示している間は、標準エラーに何も書かない（画面が崩れる）。作成・移動のメッセージと移動先の書き出しは、端末を元に戻してから行う。
- 並び順の既定は**最近使った順**。`Ctrl+S` で「最近使った順 → 作成日順 → 名前順」と切り替える。検索語があるときは一致度の順で、同点ならこの並び順に従う。
- 使用履歴は作業ルートではなく、ローカルの状態ディレクトリ（`<data_local_dir>/workroom/history.json`）に保存する。クラウド同期されたルートに書くと、移動のたびに同期が走るため。履歴にない部屋は、作成日を代わりに使う。
- 検索は slug・案件・説明を対象にしたファジー検索（nucleo-matcher）。
- キー操作：

| キー | 動作 |
|---|---|
| 文字入力 / Backspace / `Ctrl+U` | 検索語の入力 / 1文字削除 / 全消去 |
| `↑↓` / `Ctrl+P` `Ctrl+N` / `Ctrl+K` `Ctrl+J` | 選択の移動 |
| `Enter` | 移動する。「+ 新規作成」の行なら、検索語を slug にして作成する |
| `Ctrl+D` | 完了の切り替え（削除はしない） |
| `Ctrl+O` | ファイラーで開く |
| `Ctrl+S` | 並び順の切り替え |
| `Tab` | 完了済みの表示・非表示 |
| `Alt+P` | プレビューの表示・非表示 |
| `Esc` / `Ctrl+C` | 終了 |

- プレビュー：README の先頭数行、ファイル構成（深さ2）、ファイル数と合計サイズ。
  - サイズはメタデータだけを合計し、中身は読まない（オンデマンド同期のドライブでもダウンロードが発生しない）。
  - 計算は別スレッドで行い、結果はキャッシュする。
- アイコンは Nerd Font を使う。設定 `icons` で有効にする（既定はオフ。使っている端末のフォントが分からないため）。
- Windows の crossterm は、キーを押したときと離したときの両方でイベントを送る。`KeyEventKind::Press` だけを処理すること。

## import

既存のディレクトリを部屋として取り込む。

1. 日付は `--date`、なければ名前の先頭の `YYYY-MM-DD`、それもなければ作成日を使う。slug は引数で指定するか、名前から作る（小文字にし、`_` と空白を `-` に置き換える）。
2. コピーしてファイル数を照合する。README.md と AGENTS.md がなければ作る。
3. Claude Code の `<claude_dir>/projects/<旧パス由来の名前>/` を `<新パス由来の名前>/` にコピーする。jsonl 内の `"cwd":"<旧パス>"` を新しいパスに書き換える（Windows 形式と `/c/...` 形式の両方。後ろが `"` `\` `/` のものだけを対象にする）。元の履歴は残す。
4. 移行元を削除する。

**Claude Code の非公式な内部形式に依存している**ので、形式が変わると動かなくなる可能性がある。README にもその旨を明記する。

## 設定

`$XDG_CONFIG_HOME/workroom/config.toml`、未設定なら `~/.config/workroom/config.toml`（Windows でも同じ）。

```toml
root = '~/workroom'          # 作業ルート
day_start_hour = 0           # 5 にすると 4:59 までは前日の日付で作成する
recent_days = 14             # ls の既定の表示日数
icons = false                # Nerd Font のアイコン
exclude = ["node_modules", ".venv", "__pycache__", ".git"]
# agent_rules = '<root>/_workroom/AGENTS.md'
```

外部の場所はすべて環境変数で差し替えられる。テストではすべて一時ディレクトリに向けること。

| 環境変数 | 用途 |
|---|---|
| `WORKROOM_CONFIG` | 設定ファイル |
| `WORKROOM_ROOT` | 作業ルート |
| `WORKROOM_STATE_DIR` | 使用履歴などの状態 |
| `WORKROOM_CLAUDE_DIR`（なければ `CLAUDE_CONFIG_DIR`） | Claude Code のデータ（既定 `~/.claude`） |
