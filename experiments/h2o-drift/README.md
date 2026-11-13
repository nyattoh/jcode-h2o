# h2o-drift 試作品と独立契約試験

第1段階（decision01、Kota承認 2026-11-10 19:38 JST）。
このcrateは独立workspaceのまま、公開APIの契約を外部接続なしで試験する。
本体の実装、ルートworkspace、共通Cargo設定、scriptsは変更しない。

## 再現

リポジトリのルートから、次のコマンドを専用のPowerShellプロセス内で実行する。
`RUSTC_WRAPPER` の変更はそのプロセスだけに適用する。

```powershell
$env:RUSTC_WRAPPER=''
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift
```

2026-11-10、Windowsで既存3件と追加12件の計15件が成功（exit 0）。
今回は契約試験の追加のみであり、失敗試験から製品実装を修正したRED/GREEN記録ではない。

## 観測する契約

- 空・全バイト値・日本語・64 KiBの原文を各相で取得でき、保存原文も一致する。
- `Memory::open` で新しいインスタンスを作っても原文を取得できる。常駐コピーは引き継がない。
- 同一IDへの再挿入を同じインスタンスと別インスタンスから順に試し、衝突を通知して原文とキャッシュを保持する。
- 不正ID、存在しない原文、保存後に取り除いた原文、ディレクトリの代わりに渡したファイルの失敗を通知する。
- 相遷移と取得時の論理常駐ペイロード量を確認し、返却コピーの変更が原文へ波及しない。
- 修正予算ゼロ、unknownの連続、回復後の予算枯渇、停止の不可逆性、枝間の独立性を確認する。

## 第1段階時点の未検証範囲

再openは同一プロセス内の試験であり、別プロセスの再起動・クラッシュ復旧・電源断の証拠ではない。
ID衝突試験は逐次実行であり、同時書込みや書込み途中の読取りは扱わない。
復元失敗試験は原文の不在を扱い、改ざん・破損検出や権限障害を保証しない。
`resident_bytes` は論理ペイロード量であり、RSSやエネルギーの測定ではない。

unknownは呼出側から直接与える（第1段階時点）。Jev故障をunknownへ変換するadapterや、
Swarm上の枝停止・履歴保持、実daemon、実bot、SemEvolve、外部サービスは未接続・未検証。
未測定のRSS、I/O、レイテンシ、トークン、費用、実環境での効果はunknown。

第1段階は完了。2026-11-10 21:17 JSTに第2〜3段階の継続承認を受けた。
第2段階の実装・試験結果と、上記の未検証範囲から進んだ点は
[`STAGE02.md`](../../docs/vis-harness/STAGE02.md)を参照する。第4段階の手前で停止する。
第3段階のSDK境界の模擬監視・再現コマンド・検証範囲は
[`STAGE03.md`](../../docs/vis-harness/STAGE03.md)を参照する。
SDK接続を含む試験は同じコマンドに `--features sdk` を追加する。
追加承認後の反例試験・プロセス中断・長時間試験は
[`STAGE03_REINFORCEMENT.md`](../../docs/vis-harness/STAGE03_REINFORCEMENT.md)、
ルーターと監視の設計境界は
[`CONTROL_BOUNDARIES.md`](../../docs/vis-harness/CONTROL_BOUNDARIES.md)を参照する。

長時間の公開API競合試験は通常のsuiteから除外している。専用PowerShellプロセスで明示的に実行する。
時間は1〜1800秒に制限し、初期値は5秒。read probe数とpayload byte数はRSS・OS物理I/O・性能評価ではない。

```powershell
$env:RUSTC_WRAPPER=''
$env:H2O_SOAK_SECONDS='180'
cargo test --offline --release --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-storage-soak --test storage_soak concurrent_publication_soak -- --ignored --nocapture
```

## 第4段階（decision02）

2026-11-11、Kotaが実Jev判定器とjcode Swarm接続（選択肢1と2）を承認した。範囲は実装とoffline／loopback試験だけ。
上記「Jev故障をunknownへ変換するadapter」は、feature `jev` の `JevJudge` として実装し、127.0.0.1の試験serverでだけ試験した。
Swarm接続はbinary `h2o-monitor` として実装し、メモリ内の模擬peerと、存在しないsocketへの接続失敗でだけ試験した。
実Jevへの送信は2026-11-11に合成データで1回だけ（Kota承認、Python参照client経由）。Rust transportでの実送信と実daemonでの実行は未承認・未実行。契約・結果・手動手順・未検証一覧は
[`STAGE04.md`](../../docs/vis-harness/STAGE04.md)を参照する。

```powershell
$env:RUSTC_WRAPPER=''
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features jev
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features "sdk jev"
```

2026-11-11、Windowsでいずれも71件成功・ignored 5（exit 0。ignoredのうち1件は実Jev呼出の `tests/jev_live.rs`）。

独立workspaceの継続を提案し、本体への接続段階でworkspace追加の要否を再評価する。

## ローカルUIランチャー（`h2o-ui`）

feature `ui`（`sdk` を含む）で有効になる、127.0.0.1専用の静的サーバー。デーモン・実モデル・ブラウザ自動起動はなく、外部通信もしない
（待受と試験は同一マシンのloopback TCPを使うので「ネットワークなし」ではなく「外部通信なし」）。
UIは既定で「デモ／未接続」表示で、実際のH₂O状態は取り込んだスナップショットだけを読み取り専用で表示する。

コマンドは **Git Bash** で実行する（Windowsでは `RUSTC_WRAPPER` を空にする必要がある。PowerShellの `$env:RUSTC_WRAPPER=''` は変数を削除するだけで、
プロジェクト設定のwrapperが再び使われるため使わない）。リポジトリのルートで、角括弧なしでそのまま実行できる:

```bash
export RUSTC_WRAPPER=
cargo run --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features ui --bin h2o-ui -- --port 8787
```

PowerShellから実行する場合は同じ内容をGit Bash経由にする:

```powershell
& 'C:\Program Files\Git\bin\bash.exe' -lc 'export RUSTC_WRAPPER=; cargo run --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features ui --bin h2o-ui -- --port 8787'
```

引数は `--port <数値>`、`--assets <ディレクトリ>`、`--snapshot <ファイル>`（すべて省略可。`--` の後ろにそのまま足す。例: `-- --port 0 --snapshot my-snapshot.json`）。

- `--port`: 既定 `8787`。`0` で空きポート。起動すると `http://127.0.0.1:<port>/` を標準出力へ1行出す（ブラウザは自分で開く）。ホスト指定フラグはない。
- `--assets`: 既定は `experiments/h2o-drift/ui`。配信するのは `/`・`/app.css`・`/core.js`・`/app.js` の固定ルートだけで、リクエストのパスからファイルを探すことはない。
  ファイルは指定した（信頼できる、利用者自身が選んだ）assetsディレクトリ内の固定名から読み、シンボリックリンク／ジャンクションは辿る。ディレクトリ封じ込めは保証しない。
- `--snapshot`: 明示的に指定したJSONファイルを起動時に1回だけ読み、実際の `BranchSpec`/`Checkpoint`/`Receipt` 型と封筒規則（`h2o-snapshot/1`、1 MiB以下、50 branch以下、branchあたり1000 checkpoint以下、UTF-8、sequence重複なし、receiptは既存sequenceのみ、未知キー拒否）で検証する。
  封筒・branch・spec・checkpoint・receiptの各要素はJSONオブジェクト形式でなければならない（配列形式は拒否）。`captured_at` は文字列か省略で、`null` は不可。
  不正なら理由を標準エラーへ出して終了コード2で終わり、何も配信しない。有効なら `/api/snapshot` がファイルの内容をそのまま返す（未指定なら404）。`receipt: null` と `needs_reconciliation: null` は「不明」としてそのまま保持する。
  ジャーナルの自動読み取りやdaemonへの接続は行わない。
- `GET`/`HEAD` のみ。`Host` ヘッダがちょうど1つで、`127.0.0.1:<port>` / `localhost:<port>` でなければ403（重複Hostも403）。
  ルートは生のrequest-targetとの完全一致で、`/app.css?x` や `/#x` のようなクエリ・フラグメント付きは404（GET/HEADとも）。
  リクエストヘッダは終端を含めて8 KiB（8192 byte）・3秒まで（超過は431／408）。`HEAD` にはエラー応答でも本文を付けない。CSPなどのセキュリティヘッダは全応答に付く。
- 引数エラー（不明・重複・値なし・不正なport・存在しない `--assets`）は終了コード2。ポート使用中などの待受失敗は終了コード1。
- ランチャーを使わず `ui/index.html` を `file://` で直接開いても動く。その場合の実データはUIのファイル選択／貼り付けによる取り込みのみ。

試験（offline、実daemon・実Jev・外部通信なし。`tests/ui_server.rs` は一時ディレクトリの偽assetsを使う）。同じくGit Bashで:

```bash
export RUSTC_WRAPPER=
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features ui
```

注意: 一部のWindows環境にはloopbackのHTTP通信を書き換えるフィルタがあり、標準メソッドのリクエストの2つ目以降の `Host` ヘッダを黙って落とし、
未完成のヘッダ（最初の行が終わったあと）をサーバーへ渡さない（素のPythonソケットサーバーでも再現。ランチャーの不具合ではない）。
このため重複Hostの試験は、非標準メソッド（`XGET`）のTCP試験と、TCPを介さない `ui::tests`（GET/HEAD/POSTを含む）で行う。

## ネイティブ受付UI（Rust / egui、`native-ui` feature）

ブラウザを使わないRustネイティブの受付画面（`h2o-native`）。会話・セッション・デモ応答・スナップショット表示はすべてRust（`src/native.rs` の状態、`src/native_ui.rs` の egui 表示）。
既定・`sdk`・`jev`・`ui` featureの挙動は変わらない（eframe 0.37 は `native-ui` を指定したときだけ取得・ビルドされる）。Git Bashでリポジトリのルートから:

```bash
export RUSTC_WRAPPER=
cargo run --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-native --features native-ui --bin h2o-native
```

初回だけ crates.io から eframe/egui 一式を取得する（以降は `--offline` 可）。`--target-dir target/h2o-native` は `h2o-ui` 用の `target/h2o-drift` と分けて、実行中の実行ファイルのロックを避けるため。

- 引数は `--snapshot <ファイル>` だけ（省略可。不明な引数は終了コード2）。指定したファイルを起動時に1回、受付と同じ `ui::load_snapshot` で検証して監視パネルへ取り込む。読めない・不正な場合は画面にエラーを出し、取り込まない。
- 表示は常に「デモ・未接続」。モデル・Jev・daemon・ネットワークへは接続せず、送信すると入力から決まる固定のデモ応答（約0.6秒の生成中表示つき）を返す。`/demo-error` は擬似エラー表示の確認用。チャットからファイル編集や実行はしない（作業先を確定しても同じ）。
- 履歴（新しいチャット・選択）はこのウィンドウのメモリ内のみで、保存されない。eframeの永続化featureも無効。
- Enterで送信、Shift+Enterで改行。IMEの変換中・確定時に送信しない処理はeguiのヘッドレス試験で確認した。修正前の実装者によるWindows実IMEの観察は参考記録（evidence/N1.md）に限り、修正後の実窓・実IME操作はUNKNOWN。詳細は[検証範囲](../../docs/vis-harness/NATIVE_UI.md)を参照。
- 監視パネルの取り込みは明示的なファイルパス入力か貼り付けJSONのみ（自動でジャーナル等は読まない）。検証は受付と同じゲート（`h2o-snapshot/1`、1 MiB以下ほか上記 `--snapshot` の規則）。
  不正な取り込みは表示中の内容を変えずにエラーだけを出す。型エラーのメッセージは値を含みうるため、種類だけを表示する。
  observationの output/error と context のバイト列は保持も表示もしない。`receipt: null` / `needs_reconciliation: null` は「不明（null）」、receipt未記載は「記録なし」。文字列はそのまま文字として表示する（制御文字・双方向制御文字は置き換え）。
- 作業先（第5段階）: 監視パネルの「作業先」欄で、フォルダの絶対パスを入力し、`読み取り専用`／`読み書き` を明示的に選んで `確定` する（権限の既定はなく、選ぶまで確定できない）。存在しない・フォルダではない・読み取れない場合はそれぞれのエラーを出し、フォルダは作らない。確定すると状態行が `作業対象: 確定済み: <パス> · <権限>`（1行・省略あり）になり、`計画（未実行）` の行が出るが、作業セッションは作らない。パスか権限を変えると確認待ちに戻る。受付チャットのセッションは変わらない（中立の空フォルダ・ツール無効・自動承認なし）。確定した作業先は保存しない。詳細は[第5段階](../../docs/vis-harness/STAGE05.md)。
- 各行には由来ラベル（事実／デモ／不明／取込／計画）を付ける。取込データは読み取り専用で、現在の実状態とは限らない。
- 幅に応じて3段: 1180pt以上は履歴・チャット・監視の3列、760pt以上は監視パネルを見出しのボタンで開閉、それ未満は「チャット／履歴／監視」のタブ。状態行は1行で省略（…）、本文は折り返す。最小ウィンドウは360×480pt。
- 日本語フォントは同梱しない。既知のシステムフォント（Windows: Yu Gothic → メイリオ → MS ゴシック、macOS: ヒラギノ角ゴシック W3、Linux: Noto Sans CJK の代表的なパス）の最初に読めたものを使い、監視パネルに表示する。
  見つからない場合は日本語が豆腐（□）になり、その旨を英語併記で表示する。日本語フォントでは `\` が `¥` の字形で表示される（文字自体は `\`）。

試験（offline、ウィンドウを開かずにeguiの実際のTextEdit・IMEイベント・レイアウトを通す。実daemon・実Jev・外部通信なし）:

```bash
export RUSTC_WRAPPER=
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-native --features native-ui
```
