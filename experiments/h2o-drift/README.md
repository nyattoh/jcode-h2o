# h2o-drift 試作品と独立契約試験

第1段階（decision01、Kota承認 2026-11-10 19:38 JST）から第5段階までを収めた独立 Cargo workspace です。
jcode 本体の crate（`src/`、`crates/`）やルートの `Cargo.toml`・`.cargo/config.toml` は変更していません。

## Windows での実行方法（`scripts/h2o.ps1` と `RUSTC_WRAPPER` の注意）

リポジトリルートの `.cargo/config.toml` は POSIX シェルスクリプト `scripts/rustc-parallel-frontend` を `rustc-wrapper` に指定しています。そのため Windows でそのまま `cargo` を実行すると `os error 193` になります。また、PowerShell で `$env:RUSTC_WRAPPER=''` と書くと環境変数が空文字ではなく**未定義（削除）**になり、`.cargo/config.toml` の設定が再び使われて失敗します（Windows PowerShell 5.1 では `$env:CARGO_BUILD_RUSTC_WRAPPER=''` も同様に削除されます）。

リポジトリのルートから、次のいずれかの方法で実行してください:

1. **PowerShell ヘルパースクリプト（推奨、Windows PowerShell 5.1 / PowerShell 7+ 両対応）**:
   [`../../scripts/h2o.ps1`](../../scripts/h2o.ps1) は子 `cargo` プロセスの `ProcessStartInfo` 環境変数に `CARGO_BUILD_RUSTC_WRAPPER=""` を直接設定するため、Windows PowerShell 5.1 と PowerShell 7+ の両方でそのまま動作します（秘密情報の保存や有料 API の自動呼出は行いません）。
   ```powershell
   .\scripts\h2o.ps1 help          # サブコマンドとオプション一覧
   .\scripts\h2o.ps1 test          # offline Rust 試験（native-ui+jev / default）+ Web UI JS 試験
   .\scripts\h2o.ps1 clippy        # cargo fmt --check + cargo clippy --all-features --all-targets -D warnings
   .\scripts\h2o.ps1 build-release # target/h2o-native/release/ に 3 バイナリを release ビルド
   .\scripts\h2o.ps1 run-native    # h2o-native 起動（ビルド済み release を優先。ソース変更後は build-release 再実行か -Cargo）
   .\scripts\h2o.ps1 run-ui        # h2o-ui 起動（既定 Port 8787、ビルド済み release を優先）
   .\scripts\h2o.ps1 serve-daemon  # jcode serve をフォアグラウンド起動
   .\scripts\h2o.ps1 run-monitor --help
   ```
   初回のみ crates.io から依存取得が必要な場合は `-Online` を付けます（既定は `--offline`）。`run-native` / `run-ui` / `run-monitor` は `target/h2o-native/release/` のビルド済みバイナリを優先して起動するため、Rust ソースを変更した後は `.\scripts\h2o.ps1 build-release` を再実行するか `-Cargo` を指定してください。

2. **PowerShell から `h2o.ps1` を使わず直接 `cargo` を呼ぶ場合**:
   `--config "build.rustc-wrapper=''"` を渡します（PowerShell 5.1 / 7+ 共通で有効。PowerShell 7+ では `$env:CARGO_BUILD_RUSTC_WRAPPER = ""` も有効）。

3. **Git Bash / Linux / macOS から呼ぶ場合**:
   `export RUSTC_WRAPPER=` を設定してから `cargo` を実行します。

## 再現（基本契約試験）

リポジトリのルートから実行する:

```powershell
# PowerShell（ヘルパー経由: native-ui+jev と既定 feature の両方）
.\scripts\h2o.ps1 test

# PowerShell（既定 feature のみ直接実行）
$env:CARGO_BUILD_RUSTC_WRAPPER = ""
cargo --config "build.rustc-wrapper=''" test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift
```

Git Bash の場合:

```bash
export RUSTC_WRAPPER=
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift
```

- 2026-11-10（第1段階時点）: Windows で既存 3 件と追加 12 件の計 15 件が成功（exit 0）。第1段階は契約試験の追加のみであり、失敗試験から製品実装を修正した RED/GREEN 記録ではない。
- 2026-11-14（最新確認、Windows）:
  - 既定 feature: **38 passed, 0 failed, 3 ignored**
  - `--features sdk`: **102 passed, 0 failed, 4 ignored**
  - `--features "sdk jev"`（`--features jev` と同等）: **122 passed, 0 failed, 5 ignored**
  - `--features ui`: **126 passed, 0 failed, 4 ignored**
  - `--features native-ui`: **166 passed, 0 failed, 4 ignored**
  - `--features "native-ui jev"`: **189 passed, 0 failed, 5 ignored**（CLI 引数試験 `tests/h2o_native_bin.rs` 3件含む）
  - `--features "native-ui,embedded-runtime" --test embedded_runtime`: **3 passed, 0 failed**（組み込みランタイムライフサイクル・状態遷移・コントローラーハンドシェイクとツール除外）
  - Web UI JS 試験（`node --test experiments/h2o-drift/ui/tests/*.test.mjs`）: **28 passed, 0 failed**
  - `cargo fmt --manifest-path experiments/h2o-drift/Cargo.toml -- --check` および `cargo clippy --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-native --all-features --all-targets -- -D warnings`: **exit 0（warning 0 件）**
  - リリースバイナリ（`target/h2o-native/release/h2o-native.exe`）: **87.6MB, PE Subsystem = 2 (IMAGE_SUBSYSTEM_WINDOWS_GUI), 親コンソール `--help` (exit 0)**

## 観測する契約

- 空・全バイト値・日本語・64 KiBの原文を各相で取得でき、保存原文も一致する。
- `Memory::open` で新しいインスタンスを作っても原文を取得できる。常駐コピーは引き継がない。
- 同一IDへの再挿入を同じインスタンスと別インスタンスから順に試し、衝突を通知して原文とキャッシュを保持する。
- 不正ID、存在しない原文、保存後に取り除いた原文、ディレクトリの代わりに渡したファイルの失敗を通知する。
- 相遷移と取得時の論理常駐ペイロード量を確認し、返却コピーの変更が原文へ波及しない。
- 修正予算ゼロ、unknownの連続、回復後の予算枯渇、停止の不可逆性、枝間の独立性を確認する。

## 第1〜3段階の経過と長時間競合試験

第1段階時点の未検証範囲（同一プロセス内再open、逐次ID衝突、不在原文のみの復元失敗、論理ペイロード量 `resident_bytes` 等）から進んだ第2〜3段階の実装・試験結果は次を参照する:

- 第2段階（原子的公開 `fsync` + `hard_link`、プロセス間競合・強制終了試験）: [`STAGE02.md`](../../docs/vis-harness/STAGE02.md)
- 第3段階（SDK境界の模擬監視・ジャーナル記録・再現コマンド）: [`STAGE03.md`](../../docs/vis-harness/STAGE03.md)（`--features sdk`）
- 第3段階補強（反例試験・プロセス中断・状態空間網羅・長時間試験・Windows予約デバイス名拒否）: [`STAGE03_REINFORCEMENT.md`](../../docs/vis-harness/STAGE03_REINFORCEMENT.md)
- ルーターと監視の設計境界: [`CONTROL_BOUNDARIES.md`](../../docs/vis-harness/CONTROL_BOUNDARIES.md)

長時間の公開API競合試験は通常のsuiteから除外している。時間は1〜1800秒に制限し、初期値は5秒。read probe数とpayload byte数はRSS・OS物理I/O・性能評価ではない。

```powershell
$env:CARGO_BUILD_RUSTC_WRAPPER = ""
$env:H2O_SOAK_SECONDS = "180"
cargo --config "build.rustc-wrapper=''" test --offline --release --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-storage-soak --test storage_soak concurrent_publication_soak -- --ignored --nocapture
```

## 第4段階（decision02）と Swarm 監視 CLI（`h2o-monitor`）

2026-11-11、Kotaが実Jev判定器とjcode Swarm接続（選択肢1と2）を承認した。

- Jev故障・低確率を `unknown`（保留）へ変換するアダプタは feature `jev` の `JevJudge` として実装し、`127.0.0.1` の試験HTTPサーバー（15件）に加えて次の単発実送信を確認した:
  - 2026-11-11: Python参照クライアント経由で合成データを1回送信（`jev-1.14.0`、`aligned`）。
  - 2026-11-14: Rust `UreqTransport`（`rustls`）の `tests/jev_live.rs`（`H2O_JEV_LIVE=1`、`#[ignore]`）を1回実行して成功（`1 passed`、`signal=Aligned reason=jev aligned p=0.850 model=jev-1.14.0 confidence=0.770 input_tokens=573 output_tokens=47`）。
- Swarm接続は binary `h2o-monitor` として実装し、メモリ内の模擬peerと、存在しないsocketへの接続失敗で試験した。**実 `jcode` daemon / Harness API bridge との一貫実行や、`misaligned`・エラー応答の実Jev観測は未検証（unknown）**。契約・詳細・未検証一覧は [`STAGE04.md`](../../docs/vis-harness/STAGE04.md) を参照する。

```powershell
# PowerShell（ヘルパー経由）
.\scripts\h2o.ps1 test -Features "sdk jev"

# PowerShell（直接実行）
$env:CARGO_BUILD_RUSTC_WRAPPER = ""
cargo --config "build.rustc-wrapper=''" test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features jev
cargo --config "build.rustc-wrapper=''" test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features "sdk jev"
```

### 実会話 / Swarm 監視の前提（Harness API ブリッジと既定ソケットパス）

`h2o-native`（`実会話` モード）および `h2o-monitor` は、`jcode serve` の内部ソケットではなく **Harness API ソケット**へ接続する（どちらも `ensure_runtime: false` のため daemon やブリッジを自動起動しない）。

1. **起動が必要な2プロセス**:
   - `jcode serve`（例: `.\scripts\h2o.ps1 serve-daemon`）
   - Harness API ブリッジ: 対応版 `jcode` の `jcode api-bridge`（注: 配布版 `jcode` v0.99.1 には未搭載）、または [`../../crates/jcode-harness-api-server`](../../crates/jcode-harness-api-server) からオンラインでビルドした `jcode-harness-api-bridge [API_SOCKET] [LEGACY_SOCKET]`（git 依存取得のためネットワークが必要。例: Git Bash で `export RUSTC_WRAPPER=; cargo build -p jcode-harness-api-server --bin jcode-harness-api-bridge`）。
2. **Windows での既定 Harness API ソケットパス**:
   - **`%TEMP%\jcode-<USERNAME>\jcode-api.sock`**（PowerShell では `"$env:TEMP\jcode-$env:USERNAME\jcode-api.sock"`。環境変数 `JCODE_API_SOCKET` / `JCODE_RUNTIME_DIR` で上書き可）。このパスを `h2o-native` のエンドポイント欄へ入力するか、`h2o-monitor` の `-Socket`（`--socket`）へ指定する。
3. **検証状態**: Windows 実環境における実会話（Real Chat）および `h2o-monitor` の End-to-End 実行は **未検証（UNVERIFIED）**。

### `h2o-monitor` の起動とサンプル定義（`examples/sample-spec.json`）

`h2o-monitor`（`required-features = ["sdk", "jev"]`）は、上記の起動済み Harness API ソケット上の1つの worker セッションを監視し、完了したツール実行ごとに Jev で目的整合性を判定する。環境変数 `TYPESAFE_API_KEY` を呼出時に読む（表示・ログ保存はしない）。

`BranchSpec` のサンプルとして [`examples/sample-spec.json`](examples/sample-spec.json) を用意している。実運用時は `owner_session_id` / `session_id`（および CLI の `--owner` / `--worker`）を実際の親セッションID・直下 worker セッションIDに揃えること（不一致は接続前に拒否される）。

```powershell
# ヘルプ表示（exit 0）
.\scripts\h2o.ps1 run-monitor --help

# 実行例（実Jev送信と実Harness API接続を伴う・UNVERIFIED）
.\scripts\h2o.ps1 run-monitor -EnableJev `
  -Socket "$env:TEMP\jcode-$env:USERNAME\jcode-api.sock" `
  -Spec experiments\h2o-drift\examples\sample-spec.json `
  -Journal target\h2o-journal-run1 `
  -Sources <existing-sources-dir> `
  -Owner replace-with-owner-session-id `
  -Worker replace-with-worker-session-id `
  -PollMs 200
```

- 必須フラグ: `--enable-jev`（または `-EnableJev`）、`--socket`、`--spec`、`--journal`（run/branch 専用ディレクトリ、未作成なら作成）、`--sources`（既存の H₂O `Memory` ディレクトリ）、`--owner`、`--worker`。
- 任意フラグ: `--jev-base-url <url>`（既定 `https://api.typesafe.ai`）、`--min-probability <p>`（既定 `0.6`）、`--deadline-ms <ms>`（既定 `10000`）、`--output-cap-bytes <n>`（既定 `4096`）、`--include-context-bytes`、`--poll-ms <ms>`（既定 `200`）、`-h` / `--help`。
- 終了コード: `0` = `Stopped` / `Disconnected`（照合待ちなし）、`3` = `NeedsReconciliation`（接続断や制御要求失敗で要照合）、`2` = 引数・spec・設定・接続エラー。

## ローカルUIランチャー（`h2o-ui`）

feature `ui`（`sdk` を含む）で有効になる、`127.0.0.1` 専用の静的サーバー。デーモン・実モデル・ブラウザ自動起動はなく、外部通信もしない
（待受と試験は同一マシンのloopback TCPを使うので「ネットワークなし」ではなく「外部通信なし」）。
Web UI（`experiments/h2o-drift/ui/`）は既定で「デモ／未接続」表示で、実際のH₂O状態は取り込んだスナップショットだけを読み取り専用で表示する。

PowerShell ヘルパーから起動する場合:

```powershell
.\scripts\h2o.ps1 run-ui -Port 8787
# ヘルプ表示: .\scripts\h2o.ps1 run-ui --help
```

Git Bash から直接 `cargo` で実行する場合:

```bash
export RUSTC_WRAPPER=
cargo run --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features ui --bin h2o-ui -- --port 8787
```

引数は `-h` / `--help`、`--port <数値>`、`--assets <ディレクトリ>`、`--snapshot <ファイル>`（すべて省略可。`cargo run` 経由では `--` の後ろにそのまま足す。例: `-- --port 0 --snapshot my-snapshot.json`）。

- `-h` / `--help`: usage を標準出力へ出して終了コード 0 で終わる。
- `--port`: 既定 `8787`。`0` で空きポート。起動すると `http://127.0.0.1:<port>/` を標準出力へ1行出す（ブラウザは自分で開く）。ホスト指定フラグはない。
- `--assets`: 既定は `experiments/h2o-drift/ui`。配信するのは `/`・`/app.css`・`/core.js`・`/app.js` の固定ルートだけで、リクエストのパスからファイルを探すことはない。
  ファイルは指定した（信頼できる、利用者自身が選んだ）assetsディレクトリ内の固定名から読み、シンボリックリンク／ジャンクションは辿る。ディレクトリ封じ込めは保証しない（ビルド済みバイナリを別場所へ移して実行する場合は `--assets` を明示指定する）。
- `--snapshot`: 明示的に指定したJSONファイルを起動時に1回だけ読み、実際の `BranchSpec`/`Checkpoint`/`Receipt` 型と封筒規則（`h2o-snapshot/1`、1 MiB以下、50 branch以下、branchあたり1000 checkpoint以下、UTF-8、sequence重複なし、receiptは既存sequenceのみ、未知キー拒否）で検証する。
  封筒・branch・spec・checkpoint・receiptの各要素はJSONオブジェクト形式でなければならない（配列形式は拒否）。`captured_at` は文字列か省略で、`null` は不可。
  不正なら理由を標準エラーへ出して終了コード2で終わり、何も配信しない。有効なら `/api/snapshot` がファイルの内容をそのまま返す（未指定なら404）。`receipt: null` と `needs_reconciliation: null` は「不明」としてそのまま保持する。
  ジャーナルの自動読み取りやdaemonへの接続は行わない。
- `GET`/`HEAD` のみ。`Host` ヘッダがちょうど1つで、`127.0.0.1:<port>` / `localhost:<port>` でなければ403（重複Hostも403）。
  ルートは生のrequest-targetとの完全一致で、`/app.css?x` や `/#x` のようなクエリ・フラグメント付きは404（GET/HEADとも）。
  リクエストヘッダは終端を含めて8 KiB（8192 byte）・3秒まで（超過は431／408）。`HEAD` にはエラー応答でも本文を付けない。CSPなどのセキュリティヘッダは全応答に付く。
- 引数エラー（不明・重複・値なし・不正なport・存在しない `--assets`）は終了コード2。ポート使用中などの待受失敗は終了コード1。
- ランチャーを使わず `ui/index.html` を `file://` で直接開いても動く。その場合の実データはUIのファイル選択／貼り付けによる取り込みのみ。

試験（offline、実daemon・実Jev・外部通信なし。`tests/ui_server.rs` は一時ディレクトリの偽assetsを使う）:

```powershell
.\scripts\h2o.ps1 test -Features ui
```

```bash
export RUSTC_WRAPPER=
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features ui
```

注意: 一部のWindows環境にはloopbackのHTTP通信を書き換えるフィルタがあり、標準メソッドのリクエストの2つ目以降の `Host` ヘッダを黙って落とし、
未完成のヘッダ（最初の行が終わったあと）をサーバーへ渡さない（素のPythonソケットサーバーでも再現。ランチャーの不具合ではない）。
このため重複Hostの試験は、非標準メソッド（`XGET`）のTCP試験と、TCPを介さない `ui::tests`（GET/HEAD/POSTを含む）で行う。

## ネイティブ受付UI（Rust / egui、`native-ui` feature）

ブラウザを使わないRustネイティブの受付画面（`h2o-native`）。会話・セッション・実SDKチャット制御・デモ応答・作業先確認・スナップショット表示はすべてRust（`src/native.rs`、`src/native_ui.rs`、`src/chat_transport.rs`、`src/work_target.rs`）。
既定・`sdk`・`jev`・`ui` featureの挙動は変わらない（eframe 0.37 は `native-ui` を指定したときだけ使われる）。

PowerShell ヘルパーから起動する場合:

```powershell
.\scripts\h2o.ps1 run-native
# スナップショット指定: .\scripts\h2o.ps1 run-native -Snapshot <file>
# ヘルプ表示: .\scripts\h2o.ps1 run-native --help
```

Git Bash から直接 `cargo` で実行する場合:

```bash
export RUSTC_WRAPPER=
cargo run --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-native --features native-ui --bin h2o-native
```

初回だけ crates.io から依存取得が必要な場合は `--offline` を外す（`h2o.ps1` では `-Online` を付ける）。`--target-dir target/h2o-native` は実行中の実行ファイルのロックを避けるため分離している。

- **単一バイナリ完全内蔵型デスクトップアプリ（`embedded-runtime` feature）**:
  - `experiments/h2o-drift` はオプトイン feature `embedded-runtime` により、Tokio ランタイム、デーモンコアサーバー（`jcode-app-core`）、および Harness API ブリッジ（`jcode-harness-api-server`）をインプロセスに完全統合した単一の `h2o-native.exe`（release ビルドで約 87.6MB）を生成します（`.\scripts\h2o.ps1 build-release` または `cargo build --release --all-features --bins` で自動有効化）。
  - 事前に外部プロセスとして `jcode serve` やブリッジを立ち上げておく必要はありません。
- **Windows サブシステムと親コンソール usage 出力**:
  - `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` により、エクスプローラーからダブルクリック起動した際に黒いコンソール窓が一切表示されず、GUI ウィンドウのみが立ち上がります（PE Subsystem: 2 / `IMAGE_SUBSYSTEM_WINDOWS_GUI`）。
  - コマンドプロンプトや PowerShell などの親コンソールから CLI 引数（`-h`、`--help`、不正な引数）を指定して実行した際のみ、`AttachConsole(ATTACH_PARENT_PROCESS)` により親端末へ usage を出力して終了します（exit 0 / exit 2）。
- **初期表示とテーマ**: 毎回 **ライト表示** で起動する（OS のダーク設定に依存しない。ヘッダー右上の「表示: ライト / ダーク」で切替可、再起動時はライトに戻る）。詳細: [`LIGHT_MODE.md`](../../docs/vis-harness/LIGHT_MODE.md)。
- **会話モード（`実会話` 既定 ／ `デモモード` 明示選択）**:
  - **初期状態は `実会話` モード（`Kind::Real`）・未接続・エンドポイント空欄（ゼロサイドエフェクト）**。アプリを開くだけではタスク・名前付きパイプ・IPC・ネットワーク通信・ログイン・鍵の読み取りは一切発生せず、完全休眠状態が保たれます。
  - **内蔵ランタイムで接続（エンドポイント空欄で `接続`）**: エンドポイント欄を空欄のまま **`接続`** を押すと、プロセス固有の一時ディレクトリ（`h2o-embedded-{pid}-{nanos}`）内で Windows 名前付きパイプ（SHA-256 パスハッシュによる `\\.\pipe\h2o-embedded-...`）を確立し、内蔵ランタイム（デーモン＋ブリッジ）をオンデマンド起動して接続します。外部 TCP/IP ポートは一切開放せず、OS カーネルメモリ内 IPC で完結します。
  - **外部ソケットへのフォールバック（手動入力）**: エンドポイント欄に手動で外部ソケットパス（例: `%TEMP%\jcode-<USERNAME>\jcode-api.sock`）を入力して「接続」を押した場合は、内蔵ランタイムを起動せず、指定された既存ソケットへ接続します。
  - **切断および終了時のリソース回収（RAII）**: 「切断」ボタン押下時およびウィンドウクローズ（`NativeApp::drop`）時に、内蔵ランタイムのタスクを即座に `abort()` し、名前付きパイプおよび一時ディレクトリを自動消去してリソースを完全に回収します。
  - **安全不変条件**: 送信ごとに一時空フォルダ（`NeutralDir` / `h2o-native-chat` 配下）・固定 system prompt・全ツール無効（`configure_tools(enabled=[])` / `enabled=Some([]), custom=[]`）・`supports_user_response: false` の隔離セッションを作って送信します（ツール・権限要求は拒否）。2回目以降は応答済みの直近往復（質問＋応答の本文合計 32,000 Unicode 文字以内）を `system_reminder` として添えます。取込スナップショットは送りません。Windows 実環境での End-to-End 実モデルストリーミングは **未検証（UNVERIFIED）** です。詳細・制約: [`REAL_CHAT.md`](../../docs/vis-harness/REAL_CHAT.md)。
  - **`デモモード`**: 画面上部で **`デモモード`** を選ぶと、内部・外部問わず一切の通信を行わず、送信すると入力から決まる固定のデモ応答（約0.6秒の生成中表示つき）を返します。`/demo-error` は擬似エラー表示の確認用です。
  - どちらのモードでも、チャットからファイル編集やコマンド実行はしません（作業先を確定しても同じ）。
- **履歴と保存範囲**: このアプリの画面履歴はメモリ内のみでファイルへ保存しない（eframe の永続化 feature も無効）。ただし **`実会話` モードで送信した内容は接続先の jcode daemon/runtime 側に保存される場合がある**（デモモードの内容はどこにも送らない）。
- **入力と IME**: Enterで送信、Shift+Enterで改行。IMEの変換中・確定時に送信しない処理はeguiのヘッドレス試験で確認した。修正前の実装者によるWindows実IMEの観察は参考記録（evidence/N1.md）に限り、修正後の実窓・実IME操作はUNKNOWN。詳細は[`NATIVE_UI.md`](../../docs/vis-harness/NATIVE_UI.md)を参照。
- **監視パネルのスナップショット取込**: 明示的なファイルパス入力か貼り付けJSONのみ（自動でジャーナル等は読まない）。検証は `h2o-snapshot/1`（1 MiB以下ほか上記 `--snapshot` の規則）。
  不正な取り込みは表示中の内容を変えずにエラーだけを出す。型エラーのメッセージは値を含みうるため、種類だけを表示する。
  observationの output/error と context のバイト列は保持も表示もしない。`receipt: null` / `needs_reconciliation: null` は「不明（null）」、receipt未記載は「記録なし」。文字列はそのまま文字として表示する（制御文字・双方向制御文字は置き換え）。
- **作業先の確定（第5段階、`src/work_target.rs`）**: 監視パネルの「作業先」欄で、フォルダの絶対パスを入力し、`読み取り専用`／`読み書き` を明示的に選んで `確定` する（権限の既定はなく、選ぶまで確定できない）。存在しない・フォルダではない・読み取れない場合はそれぞれのエラーを出し、フォルダは作らない。確定すると状態行が `作業対象: 確定済み: <パス> · <権限>`（1行・省略あり）になり、`計画（未実行）` の行が出るが、作業セッションは作らない。パスか権限を変えると確認待ちに戻る。受付チャットのセッションは変わらない（中立の空フォルダ・ツール無効・自動承認なし）。確定した作業先は保存しない。詳細は[`STAGE05.md`](../../docs/vis-harness/STAGE05.md)。
- **由来ラベルとレイアウト**: 各行には由来ラベル（事実／デモ／不明／取込／計画）を付ける。取込データは読み取り専用で、現在の実状態とは限らない。
  幅に応じて3段: 1180pt以上は履歴・チャット・監視の3列、760pt以上は監視パネルを見出しのボタンで開閉、それ未満は「チャット／履歴／監視」のタブ。状態行は1行で省略（…）、本文は折り返す。最小ウィンドウは360×480pt。
- **フォント**: 日本語フォントは同梱しない。既知のシステムフォント（Windows: Yu Gothic → メイリオ → MS ゴシック、macOS: ヒラギノ角ゴシック W3、Linux: Noto Sans CJK の代表的なパス）の最初に読めたものを使い、監視パネルに表示する。
  見つからない場合は日本語が豆腐（□）になり、その旨を英語併記で表示する。日本語フォントでは `\` が `¥` の字形で表示される（文字自体は `\`）。

試験（offline、ウィンドウを開かずにeguiの実際のTextEdit・IMEイベント・レイアウトを通す。実daemon・実Jev・外部通信なし）:

```powershell
.\scripts\h2o.ps1 test -Features native-ui
```

```bash
export RUSTC_WRAPPER=
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-native --features native-ui
```
