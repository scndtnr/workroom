# ロードマップ

これは開発の記録である。作者の環境（端末の Herdr など）での確認の結果や、作者と AI エージェントで決めた経緯も、そのまま残している。仕様は `docs/design.md` と README を参照すること。

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

## M3 プレビュー・サイズ・アイコン（済）
実装は Codex（`agent-handoff`）、レビューとコミットは Claude Code。
- [x] プレビューを別スレッドで作り、キャッシュする（README → tree → サイズの順に届いたものから表示。`tui/preview.rs`）
  - 新しい要求を優先し、古い要求は読み飛ばす。サイズの計算中に選択が変わったら打ち切る（世代番号で判定。打ち切った結果はキャッシュしない）。途中までの部屋は、選び直せば再要求する
  - サイズは設定の `exclude` を除いて数える（`fsinfo::disk_usage`）
  - README は先頭 10 行まで（多いと tree とサイズが欄からはみ出すため）
  - ワーカーのパニックは UI を止めず、端末の復元フックも呼ばない（標準エラーにも書かない）
  - キー待ちは `event::poll(30ms)` にして、合間に届いた結果を反映する
- [x] `icons = true` で Nerd Font のアイコンを表示する（一覧とプレビューの tree）
- Herdr（PowerShell）での対話操作をユーザーが確認した（2026-11-02）：プレビューの表示、素早いスクロール、3000 ファイルの部屋（ラグなし）

## M4 rules sync・grep・import の仕上げ（済）
実装は Codex、レビューとコミットは Claude Code。
- [x] `rules sync [--dry-run]`（`rules::is_replaceable` で判定する。手で変更された部屋は更新しない。テンプレートがなければエラー）
  - PowerShell 版が作った `<!-- wk:` で始まる AGENTS.md も「生成されたもの」とみなし、置き換えの対象にする
- [x] `grep`、`import` の統合テストを書く（Claude のデータは `WORKROOM_CLAUDE_DIR` で一時ディレクトリへ）。既存の実装に修正は不要だった
  - grep は rg がある場合（PATH になければ飛ばす）と、ない場合（PATH を空にして内蔵の検索）の両方
- 切り替えのリハーサル（2026-11-02）：本番のルートから AGENTS.md だけを一時フォルダにコピーし、ルートの AGENTS.md を `_workroom/` に置いて `rules sync` を実行した。ps1 版のコピー5部屋がすべて更新され、本文はテンプレートと一致し、2回目は「変更なし 5件」

## 追加：案件の指定方法と、日本語を含む名前（済）
実装は Codex、レビューとコミットは Claude Code。
- [x] `wk new -p 案件`（名前なし）で、案件に絞った TUI を開く。`--date` / `-c` / `-x` は作成・移動に引き継ぐ
- [x] 部屋の名前・案件名に日本語・`_`・大文字を使える。`/` は案件と部屋の区切り（`wk new EXAMPLE/調査`、TUI の検索語）
- [x] 設定 `project_uppercase`（既定 false）で、案件名の ASCII 英小文字を大文字にそろえる
- [x] Claude Code の履歴の名前の衝突を、作成・import の前に検出する（残っている履歴だけがある場合は警告）
- [x] 名前の検査を、作業ルート・テンプレートを作る前に行う
- [x] PowerShell の補完で、`names` の出力を UTF-8 として読む（日本語の部屋名のため）
- 規則の変更に合わせて、既存のテストの期待値を直した（`Bad_Slug` や日本語の名前は作れるようになった、など）
- 対話操作（案件に絞った TUI、`案件/名前` での新規作成）は未確認

## M5 公開準備（済）
進め方：Codex が 3〜5 と README・LICENSE の下書き、Claude Code が README の推敲・レビュー・公開前の確認・コミット（`agent-handoff`）。
- [x] README.md（日本語）：概要、インストール（`cargo install --git` / Releases）、シェルごとの設定（PowerShell / bash・zsh / nushell）、コマンドと TUI のキー操作、名前の規則（日本語可・`/` は案件の区切り・Claude の履歴の名前の衝突の検出）、設定と環境変数、`_workroom/AGENTS.md` と `rules sync`、注意事項（Windows で Codex を使うなら作業ルートは NTFS 上、import は Claude Code の非公式な内部形式に依存）
- [x] LICENSE（MIT。著作権者は `scndtnr`。2026-11-03 にユーザーと決定）
- [x] Cargo.toml：`rust-version = "1.88"`（ratatui 0.30 と let チェーンのため）、keywords などのメタデータ
  - Rust 1.88 で `cargo check --all-targets --locked` が通ることを確認した（CI の `msrv` ジョブでも確認する）
- [x] GitHub Actions（CI）：push / PR で Windows と Linux の `cargo fmt --check`・`cargo clippy --all-targets -- -D warnings`・`cargo test`。Linux で落ちるテストがあれば直す（WSL に Rust があれば手元で確認）。`GITHUB_TOKEN` の権限は最小（`contents: read`）
  - Linux（WSL、Rust 1.99）で fmt・clippy・テストが通った。修正は不要だった
  - CI の Windows では TEMP が 8.3 形式の短いパス（`RUNNER~1`）で、import のテストが3件落ちた。「移行元の中にいるか」の判定で、カレントディレクトリを正規化していなかった（移行元は正規化していた）ので直した。テストは一時ディレクトリを正規化して使う。手元でも短い TEMP で再現し、修正後に通ることを確認した
  - アクションはコミット SHA で固定し、`actions/checkout` は `persist-credentials: false`。`.github/dependabot.yml`（cargo と github-actions）と `SECURITY.md` も用意した
- [x] Releases：`v*` のタグの push で、Windows x64（msvc）と Linux x64（musl、静的リンク）をビルドして GitHub Releases に置く。最初は `v0.1.0`
  - タグと Cargo.toml の version が一致しなければ失敗する。各アーカイブに `.sha256` を付ける。リリースのジョブだけ `contents: write`。GitHub 上での実行は未確認
- [x] 公開前の確認：コミットの作者が noreply だけか（`git log --format='%an <%ae>'`）、全ファイルと全コミットにユーザー名・個人のフォルダ名・メールアドレスが含まれないか（`git log -p` も対象）
- [x] GitHub リポジトリ `scndtnr/workroom` を **private** で作成して push（ユーザーの確認を取ってから）。リリース（`v0.1.0`）まで行う
  - 2026-11-03：ブランチ名を `master` から `main` に変えて push した。CI が通り、`v0.1.0` のリリース（Windows の zip と Linux の tar.gz、`.sha256`）ができた。ダウンロードしてチェックサムと `--version` を確認した。Linux 版は static-pie
- [x] 保護設定（public にする前に。private の無料プランでは使えないものは public 化と同時に）
  - 済（2026-11-03、private で設定）：Dependabot alerts と security updates、`.github/dependabot.yml`（cargo と github-actions）、`SECURITY.md`、ワークフローの既定権限を読み取りのみに
  - 2026-11-04、public 化と同時に設定（private の無料プランでは API が通らなかったもの）：
    - main のルールセット「main の保護」：force push と削除の禁止、PR を通したマージと CI の通過を必須に（レビューの承認は不要、バイパスなし、ブランチの最新化は必須にしない。2026-11-03 にユーザーと決定）
      - 必須にするチェック名：`検証 (windows-latest)`・`検証 (ubuntu-latest)`・`Rust 1.88 で検証`。ジョブ名を変えると、必須のチェックが待機中のまま止まるので、ルールセットも直す
      - 有効にした後は、main に直接 push できない（ブランチ → PR → CI → マージ）
    - Secret scanning と push protection
    - Private vulnerability reporting
    - 外部からの PR のワークフローを承認制に（すべての外部の貢献者）
- [x] public 化はユーザーの確認を取ってから
  - 2026-11-04 に public にした。直前に、全ブランチ・タグ・履歴の作者と、個人のパス・メールアドレスが含まれないことを確認した

## 追加：外部の利用者に向けた見直し（済）
- [x] 内蔵のテンプレート（`templates/AGENTS.md`）を、workroom の仕組みに必要なルールだけにした。uv の使い方・ファイルの置き場所・log.md の書き方などは作者の流儀なので外し、README に「自分の流儀は `_workroom/AGENTS.md` に書き足す」と書いた
- [x] リポジトリの AGENTS.md から、作者の手元にしかないスキルに依存する「Claude Code と Codex の役割分担」の節を外した。公開前の注意を、main の保護（PR と CI）に合わせて直した
- [x] 旧 PowerShell 版（非公開）が作った `<!-- wk:` で始まる AGENTS.md を `rules sync` の置き換え対象にする処理を削除した。作者の部屋は移行済みで、外部の利用者には意味がないため。今後は手で書かれたものとして残す

## M6 確認事項と成果物ハブ
- [x] PR1：`questions.rs` のモデル・読み取り・状態の判定・追記・ロック・採番、`room::resolve_cwd`、`workroom q ask/pending/done/withdraw/show` と共通の `--room`、PowerShell の補完、確認事項の内蔵テンプレートとドキュメント
  - JSONL の警告・前方互換、件数と順番による状態、末尾の改行の補完、古いロックの回収を単体テストした。統合テストでは8プロセスの並行起票、追加回答を残す `done --upto`、会話での解決、取り下げ、部屋の解決と終了コードを確認する。実データには触れず、`ANSWERS.jsonl` はテスト側で用意する
  - 設計案の食い違い：部屋の解決はコマンド仕様の「設定が読めれば作業ルート配下だけ」を採用した。「別ルート設定でもカレントの部屋を使う」というテスト方針とは異なる。設定が読めなければ最も外側の部屋を使う
  - 回答だけにある ID は写しから表示するが、起票のない解決・取り下げはエラーにする。別ファイル間の順番は確定できないため、「取り下げた項目への回答」として表示し、取り下げの前後を断定しない
  - 2026-11-06、Windows：fmt・clippy（警告なし）・全124テスト（単体71件・既存CLI36件・q17件）が通過。Rust 1.88 の `cargo check --all-targets --locked` も通過。一時ディレクトリで起票・回答なしの解決エラー・会話での解決・取り下げ時の反映先表示・全件表示・部屋の外のエラーを確認した。検証用の回答データを用意し、`pending --room` が出す `done --upto … --room …` の行を PowerShell でそのまま実行できた。仮想ドライブと別のエージェントからの利用は未確認
- [ ] PR2：読み取りだけのハブ。既存の絞り込み・予約名検査・部屋の指定・ファイル一覧・OS で開く処理の共有、一覧・README・成果物・確認事項・アーカイブ、HTTP とパスの検査、Markdown の描画、ヘッダ、統合テスト、ドキュメントと補完
- [ ] PR3：回答の送信。`POST /api/answers`、トークン・Origin の検査、フォーム・下書き・まとめて送信・回答不要・取り下げの表示・貼り付け用の文
- [ ] PR4：仕上げ。`POST /api/open`、PDF の扱いの確定、ブラウザ・仮想ドライブ・エージェントからの利用を手で確認して結果を記録

## リリース
- `v0.1.0`（2026-11-03）：最初のリリース
- `v0.1.1`（2026-11-04）：依存の更新（toml 1.1、sha2 0.11、dirs 7）、内蔵のテンプレートを最小限に、旧 PowerShell 版への対応の削除、ヘルプの例の変更（`EXAMPLE`）

## 未決事項
- `wk grep` を TUI に統合するか（検索結果から部屋へ移動するなど）
