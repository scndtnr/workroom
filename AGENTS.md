# workroom 開発ルール（AI エージェント向け）

設計は `docs/design.md`、これまでの開発の記録は `docs/roadmap.md` を参照すること。

## 公開リポジトリとしての注意
- 公開リポジトリである。コード・テスト・ドキュメント・コミットに、個人のパス（ユーザー名を含むパス、個人のドライブ名やフォルダ名）、メールアドレス、実際の会話データを入れない。例は `~/workroom` や `C:\work` のような一般的なものにする。
- コミットの作者に個人のメールアドレスを使わない（GitHub の noreply アドレスなどを使う）。git のグローバル設定は変更しない。
- main は保護されている。変更はブランチを切って PR にし、CI（`.github/workflows/ci.yml`）が通ってからマージする。
- push・PR の作成・マージ・リリースは、ユーザーの確認を取ってから行う。

## 実装の注意
- メッセージ・ヘルプ・ドキュメントは日本語。
- テストは実データに触れない。作業ルート・設定・状態・Claude のデータは、すべて `WORKROOM_*` 環境変数か引数で一時ディレクトリに向ける。`~/.claude` に書き込むテストは書かない。
- ファイルは UTF-8（BOM なし）で書く。
- Windows の crossterm では、`KeyEventKind::Press` 以外のキーイベントを無視する。
- 描画と状態を分け、状態は単体テスト、描画は ratatui の `TestBackend` でテストする。対話操作で確認できていないことは、そうと明記して報告する。
- コミットの前に、次の確認を通す（CI と同じ）。
  - `cargo fmt --check`
  - `cargo clippy --all-targets --locked -- -D warnings`
  - `cargo test --locked`
