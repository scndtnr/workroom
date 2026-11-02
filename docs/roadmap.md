# ロードマップ

マイルストーンごとにテストを通してからコミットする。

## M0 土台（済・未コミット）
- [x] `config.rs`：設定ファイル、環境変数による上書き
- [x] `room.rs`：部屋の名前の解析、一覧（案件階層の自動判別）、作成、完了マーカー、名前の解決
- [x] `history.rs`：使用履歴（最近使った順）
- [x] `fsinfo.rs`：tree、ディスク使用量、README の先頭、アイコン
- [x] `shell.rs`：ラッパー関数（pwsh / nu / bash・zsh）、`--cwd-file` の書き出し
- [x] `ls.rs` / `grep.rs` / `import.rs`
- [x] 単体テスト 14件が通過（`cargo test`）
- `main.rs` は仮のまま（モジュールを宣言しただけ）。CLI は未実装。

## M1 CLI とシェル連携の確認
- [ ] `main.rs`：clap で CLI を作る（`--cwd-file` はグローバル引数。サブコマンドがなければ残りの引数を検索語にする：`args_conflicts_with_subcommands`）
- [ ] `init` / `root` / `new` / `ls` / `done` / `undone` / `open` / `config` / 補完用の非公開コマンド `names`
- [ ] **AGENTS.md を「案内」から「テンプレートのコピー」に変更する**（design.md の「エージェントのルール」）
  - `room::agent_pointer` と `ensure_root` を置き換える。テンプレートは `<root>/_workroom/AGENTS.md`
  - `templates/AGENTS.md` の冒頭の文言を、部屋の中に置かれる前提に直す
- [ ] 日本語を含むパスで、ラッパー経由の cd を実際に確認する（`pwsh -NoProfile` と `nu`。既存の同名関数と混ざらないよう `--cmd wr` などで試す）
- [ ] 統合テスト：`WORKROOM_*` を一時ディレクトリに向けて実行ファイルを動かす

## M2 TUI ピッカー
- [ ] 状態（検索・絞り込み・並び替え・選択）を描画から分けて実装し、単体テストする
- [ ] ratatui で描画する。`TestBackend` でスナップショットテストを数件
- [ ] キー操作（design.md の表のとおり）。`KeyEventKind::Press` のみ処理する
- [ ] 「+ 新規作成」の行、`Ctrl+D` での完了切り替え、`Ctrl+O`

## M3 プレビュー・サイズ・アイコン
- [ ] プレビューを別スレッドで作り、キャッシュする（README → tree → サイズの順に届いたものから表示）
- [ ] `icons = true` で Nerd Font のアイコンを表示する

## M4 rules sync・grep・import の仕上げ
- [ ] `rules sync`（生成元の識別子で、手で変更された部屋は更新しない）
- [ ] `grep`、`import` を CLI につなぎ、統合テストを書く（Claude のデータは `WORKROOM_CLAUDE_DIR` で一時ディレクトリへ）

## M5 公開準備
- [ ] README.md（インストール、シェルごとの設定、import が非公式形式に依存する旨）
- [ ] LICENSE（MIT）
- [ ] GitHub Actions（Windows / Linux で `cargo test`、clippy、fmt）
- [ ] Releases 用のビルド（Windows x64 / Linux x64）
- [ ] 公開前の確認：個人のパス・メールアドレスが含まれていないか（`git log --format='%an <%ae>'` と、ユーザー名・個人フォルダ名での `rg` 検索）
- [ ] GitHub リポジトリの作成と push は、ユーザーの確認を取ってから

## 未決事項
- `wk grep` を TUI に統合するか（検索結果から部屋へ移動するなど）
- 案件ごとのルールの置き場所と、連結のしかた
- PowerShell の補完：`Register-ArgumentCompleter -Native` が関数にも効くか、要確認
