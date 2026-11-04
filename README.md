# workroom

AI エージェント（Claude Code / Codex など）に、日付付きの「作業部屋」ディレクトリを用意する CLI。
Git 管理するほどでもない調査・作業の場所を、すぐに作れて、すぐに戻れて、あとから探せる状態にする。

実行ファイル名は `workroom`。シェル連携を設定すると、普段は `wk` で使える。
部屋には README.md とエージェント向けの AGENTS.md を作り、一覧には README の1行目を表示する。

```text
~/work/
├── _workroom/AGENTS.md          ルールのテンプレート
├── 2026-11-03-fix-bug/          作業部屋
│   ├── README.md
│   └── AGENTS.md
└── EXAMPLE/                     案件（任意）
    └── 2026-11-03-調査/
```

## インストール

ソースからインストールする場合は **Rust 1.88 以上**が必要。

```sh
cargo install --git https://github.com/scndtnr/workroom --locked
```

[GitHub Releases](https://github.com/scndtnr/workroom/releases) では、Windows x64（`x86_64-pc-windows-msvc`、zip）と Linux x64（`x86_64-unknown-linux-musl`、tar.gz）のバイナリを配布する。
対応するアーカイブを展開し、`workroom.exe` / `workroom` を `PATH` の通った場所に置く。バイナリの利用には Rust は不要。各アーカイブには README.md・LICENSE を含み、SHA-256 の確認用に `.sha256` ファイルを添付する。

Claude Code / Codex を起動する場合は、それぞれを別途インストールして `PATH` に通す。
横断検索は [ripgrep](https://github.com/BurntSushi/ripgrep) があれば使い、なければ内蔵の簡易検索を使う。

## シェルの設定

実行ファイルだけでは呼び出し元のシェルを移動できないため、ラッパー関数を読み込む。
`--cmd wk` で関数名を指定する（既定も `wk`）。

### PowerShell

`$PROFILE` に追加する。

```powershell
workroom init pwsh --cmd wk | Out-String | Invoke-Expression
```

### bash / zsh

bash は `~/.bashrc` に追加する。

```sh
eval "$(workroom init bash --cmd wk)"
```

zsh は `~/.zshrc` に追加する。

```sh
eval "$(workroom init zsh --cmd wk)"
```

### nushell

まずスクリプトを保存する。workroom を更新したら、このコマンドを再実行する。

```nu
workroom init nu --cmd wk | save -f ~/.workroom.nu
```

`config.nu` に追加する。`source` のパスは解析時点で決まっている必要があるため、ファイルを介して読み込む。

```nu
source ~/.workroom.nu
```

設定後はシェルを開き直す。作業ルートは設定ファイルか `WORKROOM_ROOT` で指定できる（既定は `~/workroom`）。

```sh
wk new fix-bug 不具合調査
wk new EXAMPLE/調査 案件の調査 -c
wk ls
wk
```

## コマンド

| コマンド | 動作 |
|---|---|
| `wk` / `wk <検索語...>` | 部屋を選ぶ TUI を開く。検索語があれば入力済みで開始 |
| `wk new <名前\|案件/名前> [説明...] [-p 案件] [--date D] [-c\|-x]` | 作成して移動。既に同じ日付・名前の部屋があれば移動のみ。`-c` で claude、`-x` で codex を起動 |
| `wk new -p 案件 [--date D] [-c\|-x]` | 案件に絞った TUI。選んで移動するか、「+ 新規作成」で作る |
| `wk ls [検索語] [-a\|-o] [--days N] [--depth N]` | tree 表示。既定は直近14日の未完了。`-a` で全件、`-o` で指定日数より前の未完了。検索語を指定すると日数・完了の絞り込みを外す |
| `wk grep <pattern> [rg のオプション...]` | 中身を正規表現で横断検索。rg がない場合、追加オプションは無視する |
| `wk done [名前]` / `wk undone [名前]` | 完了マーカー `.done` を付ける・外す。省略時はカレントの部屋 |
| `wk open [名前]` | ファイラーで開く。省略時はカレントの部屋、部屋の外ならルート |
| `wk root` | 作業ルートへ移動 |
| `wk import <path> [名前] [説明...] [--date D] [-p 案件]` | 既存ディレクトリと Claude Code の履歴を移行して移動 |
| `wk rules sync [--dry-run]` | 各部屋の AGENTS.md を最新のテンプレートで更新 |
| `workroom init <pwsh\|nu\|bash\|zsh> [--cmd 名前]` | シェル連携のスクリプトを出力 |
| `workroom config [--init]` | 設定の場所と内容を表示。`--init` で雛形を作成 |
| `workroom --help` / `workroom --version` | ヘルプ / バージョンを表示 |

`D` は `YYYY-MM-DD`。`new` では省略時に今日、`import` では移行元の名前の先頭の日付、なければ作成日（取得できなければ更新日）を使う。
`done`・`undone`・`open` の名前は部分一致で指定できる。候補が複数ならエラーになる。

サブコマンド名と一致しない語は TUI の検索語になる。同じ語を検索したい場合は `wk -- done`。PowerShell では `--` を引用し、`wk '--' done` と書く。
ラッパーを通さずに `workroom new` などを実行すると、移動先のパスを表示するだけで、移動やエージェントの起動は行わない。

## TUI のキー操作

検索は部屋名・案件・説明のファジー検索。既定は最近使った順で、`Ctrl+S` で作成日順・名前順に切り替える。
完了済みの部屋は、検索語があっても `Tab` を押すまで隠す。

| キー | 動作 |
|---|---|
| 文字入力 / `Backspace`・`Ctrl+H` / `Ctrl+U` | 検索語の入力 / 1文字削除 / 全消去 |
| `↑↓` / `Ctrl+P` `Ctrl+N` / `Ctrl+K` `Ctrl+J` | 選択の移動 |
| `Enter` | 部屋へ移動。「+ 新規作成」の行なら検索語を名前にして作成 |
| `Ctrl+D` | 完了の切り替え |
| `Ctrl+O` | ファイラーで開く |
| `Ctrl+S` | 最近使った順 → 作成日順 → 名前順 |
| `Tab` | 完了済みの表示・非表示 |
| `Alt+P` | プレビューの表示・非表示 |
| `Esc` / `Ctrl+C` | 終了 |

プレビューは README の先頭、深さ2のファイル構成、ファイル数と合計サイズを表示する。
TUI は標準入力・標準出力が端末のときだけ使える。パイプなどでは `wk ls` を使う。

## 名前の規則

- 部屋は `YYYY-MM-DD-<名前>`。日本語・英字・数字・`-`・`_` などを使え、英字の大文字・小文字は保つ。入力の前後の空白は除き、途中の空白の並びは `-` 1つにする。
- `/` は案件と部屋の区切り。`wk new EXAMPLE/調査` と `wk new 調査 -p EXAMPLE` は同じ指定で、併用はできない。案件は1階層だけ。
- `\ : * ? " < > |`、制御文字、先頭の `-`・`.`、末尾の `.` は使えない。案件名は、先頭の `_`、`YYYY-MM-DD-名前` の形、Windows の予約名（`CON`・`NUL`・`COM1` など。拡張子付きも）も使えない。
- `project_uppercase = true` で、案件名の ASCII 英小文字を大文字にそろえる（`example営業` → `EXAMPLE営業`）。既存のディレクトリ名は変えない。

Claude Code はパスの英数字以外を `-` に置き換えた名前で履歴を保存するため、同じ日付の `調査` と `分析` などが衝突する。
作成・import の前に既存の全部屋（完了済みを含む）と比べ、衝突したら作成せずに案内する。名前に英数字を加えて区別する。
部屋がなく履歴だけが残っている場合は作成後に警告する。履歴の名前が200文字を超える場合は、Claude 側の規則が変わるため検査しない。

## 設定

設定ファイルは `$XDG_CONFIG_HOME/workroom/config.toml`。未設定なら `~/.config/workroom/config.toml`（Windows でも同じ）。
`workroom config --init` で雛形を作れる。既存の設定ファイルは上書きしない。

```toml
root = '~/work'              # 未指定なら ~/workroom。Windows なら 'C:\work' など
day_start_hour = 0           # 0〜23。5 なら 4:59 までは前日扱い
recent_days = 14             # ls の既定の表示日数
icons = false                # Nerd Font のアイコンを使う
project_uppercase = false    # 案件名の ASCII 英小文字を大文字にそろえる
exclude = ["node_modules", ".venv", "__pycache__", ".git"]
# agent_rules = '~/work/_workroom/AGENTS.md'
```

`exclude` は tree 表示・検索・サイズ計算から除く名前。`agent_rules` は全体のルールのテンプレートの場所で、既定は `<root>/_workroom/AGENTS.md`。
設定・環境変数のパスでは、`~/work` のようにホームディレクトリからの相対指定もできる。

| 環境変数 | 用途・優先順 |
|---|---|
| `WORKROOM_CONFIG` | 設定ファイルの場所を上書き |
| `WORKROOM_ROOT` | 作業ルート。設定ファイルの `root` より優先 |
| `WORKROOM_STATE_DIR` | 使用履歴の保存先。既定は OS のローカルデータディレクトリ配下の `workroom` |
| `WORKROOM_CLAUDE_DIR` | Claude Code のデータ。未指定なら `CLAUDE_CONFIG_DIR`、それもなければ `~/.claude` |

空の環境変数は未指定として扱う。使用履歴は状態ディレクトリの `history.json` に保存する（既定は作業ルートの外）。

## エージェントのルール

初めて部屋や作業ルートを作るとき、`<root>/_workroom/AGENTS.md` がなければ[内蔵のテンプレート](templates/AGENTS.md)から作る。
このファイルを編集すると、それ以降に作る部屋の AGENTS.md に全文をコピーする。
内蔵のテンプレートは、workroom を使ううえで必要な最小限のルール（部屋の中で作業する、README の1行目を一覧の説明にする、`.done` に触らない）だけを書いている。
使うツールやファイルの置き方など、自分の流儀はこのファイルに書き足す。
案件ごとの追加ルールは `<root>/<案件>/_workroom/AGENTS.md` に置く。全体のルールの後ろに連結する。

既存の部屋への反映は次のコマンドで行う。

```sh
wk rules sync --dry-run
wk rules sync
```

先頭行の SHA-256 と本文が一致する、生成後に手で変更されていないものだけを更新する。AGENTS.md がない部屋には作り、手で変更されたものは一覧に出して残す。
`rules sync` はテンプレートを自動作成せず、なければエラーになる。作業ルート直下に AGENTS.md がある場合は `_workroom/AGENTS.md` に移して使う。親と部屋の両方を Claude Code が読むのを避けるため。

## 注意事項

- **Windows で Codex を使う場合、作業ルートは NTFS 上に置く。** クラウドの仮想ドライブでは、Codex のサンドボックスが必要とする ACL を設定できず、起動に失敗する。クラウドに残す場合は、NTFS 上のフォルダを同期・バックアップする。
- 部屋は移動せず、完了は `.done` で表す。Claude Code / Codex の会話履歴は絶対パスに紐づくため、あとから移動すると再開できなくなる。
- `import` はファイルをコピーして件数を照合し、Claude Code の履歴をコピーして `cwd` を書き換えた後、**移行元を削除する**。元の Claude の履歴は残す。大切なデータはバックアップしてから使う。シンボリックリンクなどは移行できない。
- `import` は **Claude Code の非公式な内部形式に依存する**。形式が変わると動かなくなる可能性がある。移行先の履歴が既にある場合や、履歴の名前が200文字を超える場合は、警告して履歴の移行を省く。

詳しい仕様は [docs/design.md](docs/design.md) を参照。
脆弱性の報告方法は [SECURITY.md](SECURITY.md) に記載する。

## ライセンス

[MIT](LICENSE) — Copyright (c) 2026 scndtnr
