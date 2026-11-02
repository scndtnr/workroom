# ロードマップ

マイルストーンごとにテストを通してからコミットする。

## M0 土台（済）
- [x] `config.rs`：設定ファイル、環境変数による上書き
- [x] `room.rs`：部屋の名前の解析、一覧（案件階層の自動判別）、作成、完了マーカー、名前の解決
- [x] `history.rs`：使用履歴（最近使った順）
- [x] `fsinfo.rs`：tree、ディスク使用量、README の先頭、アイコン
- [x] `shell.rs`：ラッパー関数（pwsh / nu / bash・zsh）、`--cwd-file` の書き出し
- [x] `ls.rs` / `grep.rs` / `import.rs`
- [x] 単体テスト 14件が通過（`cargo test`）

## M1 CLI とシェル連携の確認（済）
- [x] `main.rs`：clap で CLI を作る（`--cwd-file` はグローバル引数で、ヘルプには出さない）
  - `args_conflicts_with_subcommands` は使わない。`--cwd-file` も「引数」とみなされ、`wk --cwd-file f new x` の `new` が検索語になってしまうため。
    代わりに、サブコマンド名と一致しない語を検索語として受け取る。`done` などを検索したいときは `wk -- done`（PowerShell は素の `--` を自分で消費し、`wk -- done` が「カレントの部屋を完了にする」になるので、`wk '--' done` と引用する。確認済み）。
  - ヘルプは日本語（見出し・テンプレート・`-h` の説明を、サブコマンドにも再帰的に設定）。clap 自身の解析エラー（`error: unexpected argument ...`）は英語のまま。
- [x] `init` / `root` / `new` / `ls` / `done` / `undone` / `open` / `config` / 補完用の非公開コマンド `names`
  - `grep` / `import` も CLI につないだ（補完の候補に出ているため）。統合テストは M4 で書く。
  - サブコマンドなしの場合の暫定動作（検索語に1件だけ一致すれば移動）は、M2 で TUI に置き換えた。
- [x] **AGENTS.md を「案内」から「テンプレートのコピー」に変更する**（`rules.rs`。design.md の「エージェントのルール」）
  - テンプレートは `<root>/_workroom/AGENTS.md`（なければ内蔵の文面から作る）。案件の部屋には `<root>/<案件>/_workroom/AGENTS.md` を連結する
  - 1行目に `<!-- workroom:rules sha256=... -->` を入れる。`rules::is_unmodified` / `is_replaceable` を用意した（`rules sync` は M4）
  - 作業ルート直下に AGENTS.md があれば警告する（自動では移さない）
  - `templates/AGENTS.md` の冒頭の文言を、部屋の中に置かれる前提に直した
- [x] 日本語・空白を含むパスで、ラッパー経由の cd を実際に確認した（`--cmd wr`、一時ディレクトリで）
  - PowerShell 7.6（`-NoProfile`、コンソールは shift_jis）：new / root / 検索語 / 失敗時に移動しない / `-c`（偽の `claude` 関数で確認）/ 一時ファイルが残らない / 補完
    - `init` の出力に日本語のコメントがあると、shift_jis のコンソールで `Invoke-Expression` が構文エラーになった → スクリプトを ASCII のみにした（テストで固定）
    - `Register-ArgumentCompleter -Native` は関数にも効く（`TabExpansion2` で確認）
  - nushell 0.100（`nu -n`）：new / root / 検索語 / 失敗時 / `-c`（偽の `claude.cmd` で確認）/ `try` の中でも出力が流れる
    - 失敗時に関数が止まり一時ファイルが残っていた → `try` で囲んだ
  - bash（Git Bash）：new / root / 失敗時 / `-c`（PATH の先頭に置いた偽の `claude` で確認）
  - 本物の `claude` / `codex` の起動と、端末（Herdr）上での対話的な動作は未確認
- [x] 統合テスト（`tests/cli.rs`）：`WORKROOM_*` を一時ディレクトリに向けて実行ファイルを動かす（12件）
  - Google Drive for desktop の `.tmp.drivedownload` / `.tmp.driveupload` を案件として扱わないことも固定した（単体テストにも）
- ratatui を 0.30 に上げた（clap の `unicode` 機能と `unicode-width` の版がぶつかったため。TUI はまだ書いていない）

## M2 TUI ピッカー（済）
- [x] 状態（検索・絞り込み・並び替え・選択）を描画から分けて実装し、単体テストする（`tui/state.rs`）
  - 並び順の切り替え・完了済みの表示切り替えでは、選んでいる部屋をそのまま選ぶ。完了にして隠れたときは選択位置を詰める
  - 完了済みは、検索語があっても `Tab` を押すまで隠す（件数は「完了 N件非表示」と表示する）
- [x] ratatui（0.30）で描画する。`TestBackend` でスナップショットテスト（`tui/view.rs`）
  - 全角文字の幅を考えたカーソル位置（IME の変換窓の位置）もテストした
- [x] キー操作（design.md の表のとおり）。`KeyEventKind::Press` のみ処理する（`tui::action_for`。単体テストあり）
  - design.md の画面例にあった「`Ctrl+N` 新規」は、キーの表（`Ctrl+N` は選択の移動）と矛盾していたので、表に合わせた。新規作成は「+ 新規作成」の行で `Enter`
- [x] 「+ 新規作成」の行、`Ctrl+D` での完了切り替え、`Ctrl+O`
  - 作成と移動は、端末を元に戻してから `main` で行う（`ensure_root` などが標準エラーに書くため）
- [x] ラッパー経由で TUI が端末を取得できるか確認した：隠しウィンドウのコンソールで pwsh と nushell（`try { ^workroom ... }` の中）から `wr` を起動し、4秒後もキー入力待ちのまま残ることを確認（端末がなければ、すぐ終了コード 2 で戻る）
- プレビューは README の先頭だけ（同期で読み、キャッシュする）。tree とサイズは M3
- Herdr（PowerShell）での対話操作をユーザーが確認した（2026-11-02）：日本語 IME での入力、Enter での移動、`Esc` / `Ctrl+C` での終了と画面の復元、新規作成、`Ctrl+D` / `Tab` / `Ctrl+S` / `Ctrl+O` / `Alt+P`、サイズ変更、アイコン。nushell からの対話操作は未確認

## M3 プレビュー・サイズ・アイコン（済。対話操作の確認待ち）
実装は Codex（`claude-codex-handoff`）、レビューとコミットは Claude Code。
- [x] プレビューを別スレッドで作り、キャッシュする（README → tree → サイズの順に届いたものから表示。`tui/preview.rs`）
  - 新しい要求を優先し、古い要求は読み飛ばす。サイズの計算中に選択が変わったら打ち切る（世代番号で判定。打ち切った結果はキャッシュしない）。途中までの部屋は、選び直せば再要求する
  - サイズは設定の `exclude` を除いて数える（`fsinfo::disk_usage`）
  - README は先頭 10 行まで（多いと tree とサイズが欄からはみ出すため）
  - ワーカーのパニックは UI を止めず、端末の復元フックも呼ばない（標準エラーにも書かない）
  - キー待ちは `event::poll(30ms)` にして、合間に届いた結果を反映する
- [x] `icons = true` で Nerd Font のアイコンを表示する（一覧とプレビューの tree）
- **対話操作では未確認**：素早くスクロールしても固まらないか、大きな部屋でサイズが後から出るか、Herdr での表示

## M4 rules sync・grep・import の仕上げ
- [ ] `rules sync`（`rules::is_replaceable` で判定する。手で変更された部屋は更新しない）
  - PowerShell 版が作った `<!-- wk:` で始まる AGENTS.md も「生成されたもの」とみなし、置き換えの対象にする
- [ ] `grep`、`import` の統合テストを書く（Claude のデータは `WORKROOM_CLAUDE_DIR` で一時ディレクトリへ）

## M5 公開準備
- [ ] README.md（インストール、シェルごとの設定、import が非公式形式に依存する旨、「Windows で Codex を使う場合、作業ルートはクラウドの仮想ドライブではなく NTFS 上に置く」旨。design.md の「ディレクトリ構成」を参照）
- [ ] LICENSE（MIT）
- [ ] GitHub Actions（Windows / Linux で `cargo test`、clippy、fmt）
- [ ] Releases 用のビルド（Windows x64 / Linux x64）
- [ ] 公開前の確認：個人のパス・メールアドレスが含まれていないか（`git log --format='%an <%ae>'` と、ユーザー名・個人フォルダ名での `rg` 検索）
- [ ] GitHub リポジトリの作成と push は、ユーザーの確認を取ってから

## 未決事項
- `wk grep` を TUI に統合するか（検索結果から部屋へ移動するなど）
