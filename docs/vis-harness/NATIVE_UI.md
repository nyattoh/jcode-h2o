# Rustネイティブ受付UI — 2026-11-12

利用者の「Rustで実装したい」という変更により、受付UI・会話状態・デモ応答・スナップショット表示をRust（egui/eframe 0.37）で実装した。JS試作は未受入の履歴として残し、今回の成果には含めない。既存のRust HTTPランチャーとスナップショット検証器は保持した。

## 起動

リポジトリのルートからGit Bashで実行する。ウィンドウを閉じると会話履歴は消える。

```bash
export RUSTC_WRAPPER=
cargo run --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-native --features native-ui --bin h2o-native
```

初回の依存取得には公開crates.ioへのアクセスが必要。既存のgitignoreに従い、この独立crateのCargo.lockはコミット対象外。実行したlockのコピーとSHA256はタスク証拠に保存する。別環境で依存を再解決した場合は、同一lockでの再現とは区別する。

新しいチャット・履歴選択・Enter送信・Shift+Enter改行、生成中／擬似エラー／空状態を備える。常にデモ・未接続であり、実モデル・Jev・daemonへの接続も、チャットからの編集・実行もない。実作業対象は未確認のまま。履歴はメモリ内のみ。

監視画面はファイルパス／JSON貼り付け、または起動引数 `--snapshot <file>` で明示的に取り込む。既存の `ui::load_snapshot` / `validate_snapshot` と実serde型を使い、読み取り専用の表示へ変換する。観測のoutput/error本文とcontextバイト列は表示せず、null／未記載／照合不明を成功へ変換しない。不正な取り込みでは直前の表示を保持し、入力由来の未知キーや型エラー値をエラー欄へ出さない。

## 確認済みの範囲

Rust組込み試験で機能RED→GREEN→回帰を記録。独立した別ベンダーの実ファイルレビューは第1回fail後、情報露出とIMEイベント順序の問題を修正し、第2回コードレビューpass。PMの修正後試験はnative-ui 91件成功／4 ignored、native-ui,jev 110件成功／5 ignored。既存のdefault/sdk/ui/sdk,jev/ui,jev構成もオフライン回帰通過。実Jev試験はignoredのままで、今回の実Jev API呼出しはない。

## GUI確認の制限

日本語・ダーク画面・レイアウトは修正前の保存画像を目視確認した。実装者はWindows Microsoft IMEを含む実窓操作を報告したが、その操作は独自ドライバで行われ、先行する権限拒否の生ログと理由が残っていない。この記録は参考観察に限り、公式ツールによる独立GUI受入とは扱わない。READMEの「実IMEで確認済み」も、この修正前の実装者観察を指す。

公式の独立GUI操作ツールは初期化に2回失敗した。修正後バイナリの実窓操作・実IME挙動・最小360×480での操作性はUNKNOWN。修正後のIME両順序と改行は実eguiのヘッドレスフレームで検証した。日本語フォントのglyph試験は通過したが、最終バイナリの実窓での可読性を代替するものではない。Linux/macOS、他IME、アクセシビリティ、最大スナップショットの描画性能、外向き通信の実測も未確認。

利用者の変更契約は、利用不能なGUI検証をUNKNOWNとして残すことを認めている。この限定でコード・オフライン試験を受け入れ、実GUIの最終動作まで実証済みとはしない。

## 証拠

`target/2026-11-12-grok-ui/` の `state.json`、`goal-amendment-rust.md`、`rust-native-contract.md`、`evidence/N1.md`、`N1-fix1.md`、`review-N1-r1.md`、`review-N1-r2.md`、`PM-final-*.log`、`PM-fix1-*.log`、`task-gate-final.json` を参照する。targetはローカル証拠で、リモート配布物ではない。
