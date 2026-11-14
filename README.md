# jcode-h2o

[jcode](https://github.com/1jehuang/jcode) をベースにした研究用の試作です。
受付画面・JCode Swarm・H₂O Memory・Jev による目的逸脱監視を組み合わせ、
人の依頼から成果・履歴までを追跡できるエージェントハーネスを目指しています。

> **状態: 研究用試作（未完成）。** 実 daemon（Harness API bridge を含む）・実 bot・実 Swarm との一貫統合や、
> 記憶・監視の実環境での効果はまだ実証していません（Jev API については合成データ単発の疎通のみ確認済み）。
> 未確認の項目は各ドキュメントで `unknown` / `UNKNOWN` と明記しています。

## 構成

H2O 関連のコード・補助スクリプト・ドキュメントは次の場所にあります。jcode 本体の crate（`src/`、`crates/`）は upstream のまま変更していません。

| パス | 内容 |
| --- | --- |
| [`experiments/h2o-drift/`](experiments/h2o-drift/) | 試作の本体（独立した Cargo workspace）。サンプル監視定義 [`examples/sample-spec.json`](experiments/h2o-drift/examples/sample-spec.json) を同梱 |
| [`scripts/h2o.ps1`](scripts/h2o.ps1) | Windows PowerShell（5.1 / 7+）向けのビルド・試験・起動ヘルパー（`RUSTC_WRAPPER` 回避済み） |
| [`docs/vis-harness/`](docs/vis-harness/) | 要求仕様・設計・段階ごとの検証記録 |

`h2o-drift` の主な機能（Cargo の feature ごと）:

- **既定**: H₂O Memory の保存機構（原文の保存と復元、使用中・待機・ディスク退避の 3 相）、枝ごとの逸脱判定・修正予算・停止、作業先の事前確認・計画データ生成（`work_target`）
- **`sdk`**: jcode SDK（Harness API）境界での協調的チェックポイント監視・ジャーナル記録・実チャット用トランスポート
- **`jev`**: 実 Jev 判定器 `JevJudge`（`sdk` を含む。故障・低確率時は `unknown` に変換して保留）と Swarm 監視 CLI `h2o-monitor`
- **`ui`**: `127.0.0.1` 専用の静的 Web ランチャー `h2o-ui`（`sdk` を含む。スナップショットを読み取り専用で表示）
- **`native-ui`**: Rust / egui のネイティブ受付画面 `h2o-native`（`ui` を含む。**既定はライト表示・未接続の `実会話` モード**。明示選択の `デモモード`、スナップショット読込、第5段階の「作業先」フォルダ・権限の事前確定に対応）
- **`embedded-runtime`**: `h2o-native` にデーモンコア（`jcode-app-core`）と Harness API ブリッジ（`jcode-harness-api-server`）をインプロセス統合し、外部プロセス不要で即座に動作する完全自立型単一バイナリデスクトップアプリ（`h2o-native.exe`）を実現するオプトイン機能（Windows Named Pipe IPC、起動時ゼロサイドエフェクト、切断・終了時の自動クリーンアップ）

## クイックスタート

### Windows（PowerShell 5.1 / PowerShell 7 推奨）

ルートの `.cargo/config.toml` は POSIX シェルスクリプトの `rustc-wrapper` を指定しているため、Windows では [`scripts/h2o.ps1`](scripts/h2o.ps1) を使うのが最も簡単です（子 `cargo` プロセスの `ProcessStartInfo` 環境変数に `CARGO_BUILD_RUSTC_WRAPPER=""` を直接設定するため、Windows PowerShell 5.1 と PowerShell 7+ の両方で `os error 193` を防ぎます）。

```powershell
# 使い方一覧
.\scripts\h2o.ps1 help

# offline 試験一式（Rust 全主要構成 + Node があれば Web UI 試験）
.\scripts\h2o.ps1 test

# フォーマット確認 + clippy（-D warnings）
.\scripts\h2o.ps1 clippy

# release バイナリ一式を target/h2o-native/release/ にビルド（初回依存取得が必要な場合は -Online を追加）
.\scripts\h2o.ps1 build-release

# ネイティブ受付 UI を起動（target/h2o-native/release/ のビルド済みバイナリを優先。ソース変更後は build-release 再実行か -Cargo）
.\scripts\h2o.ps1 run-native

# ローカル静的スナップショット UI を起動（既定 http://127.0.0.1:8787/）
.\scripts\h2o.ps1 run-ui -Port 8787

# Swarm 監視 CLI（既存の Harness API ソケットと環境変数 TYPESAFE_API_KEY が必要）
.\scripts\h2o.ps1 run-monitor -EnableJev `
  -Socket "$env:TEMP\jcode-$env:USERNAME\jcode-api.sock" `
  -Spec experiments\h2o-drift\examples\sample-spec.json `
  -Journal target\h2o-journal-example `
  -Sources <existing-sources-dir> `
  -Owner replace-with-owner-session-id `
  -Worker replace-with-worker-session-id
```

### Git Bash / Linux / macOS（`cargo` 直接実行）

Git Bash などのシェルから直接 `cargo` を呼ぶ場合は、`export RUSTC_WRAPPER=` を設定します（PowerShell で `$env:RUSTC_WRAPPER=''` と書くと変数が削除されて `.cargo/config.toml` の wrapper が再び有効になるため、PowerShell から `h2o.ps1` を使わず直接 `cargo` を呼ぶ場合は `cargo --config "build.rustc-wrapper=''"` を使ってください）。

```bash
export RUSTC_WRAPPER=

# 試験（offline、外部通信なし）
cargo test --offline --locked --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-native --features "native-ui jev"

# ネイティブ受付 UI（初回のみ crates.io 取得が必要な場合は --offline を外す）
cargo run --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-native --features native-ui --bin h2o-native
```

### `h2o-native` の初期状態と使い方

- **単一バイナリ自立型アプリ（内蔵ランタイム）**: `h2o-native.exe`（release ビルドで約 87.6MB）は、プロセス内部に Tokio ランタイム、デーモンコアサーバー（`jcode-app-core`）、Harness API ブリッジ（`jcode-harness-api-server`）を完全統合しています。事前に別ターミナルで `jcode serve` やブリッジを立ち上げておく必要はありません。
- **初期状態（ゼロサイドエフェクト）**: **ライト表示・`実会話` モード（未接続・エンドポイント空欄）** で起動します（ヘッダーの「表示: ライト / ダーク」でテーマ切替可）。アプリを開くだけでは、タスク・名前付きパイプ・IPC・ネットワーク通信・ログイン・鍵の読み取りは一切発生せず、完全休眠状態が保たれます。Windows GUI サブシステム（`windows_subsystem = "windows"`）により、エクスプローラーからダブルクリックで起動しても黒いコンソール窓は表示されません（親コンソールから `-h` / `--help` 実行時のみ `AttachConsole` で usage を出力）。
- **すぐに画面・操作を試す（`デモモード`）**: 画面上部で **`デモモード`** を選ぶと、内部・外部問わず一切の通信を行わずに固定のデモ応答（`/demo-error` で擬似エラー表示）を確認できます。
- **内蔵ランタイムで会話する（エンドポイント空欄で `接続`）**: エンドポイント欄を空欄のまま **`接続`** を押すと、OS カーネルメモリ内の Windows 名前付きパイプ（Named Pipe `\\.\pipe\h2o-embedded-...`）上で内蔵ランタイムがオンデマンド起動し、数秒以内に接続完了（Ready）して会話可能になります。外部 TCP/IP ポートは一切開放せず、ポート衝突やファイアウォール警告もありません。全ツール無効（`configure_tools(enabled=[])`）・自動承認なし・送信ごとの空一時フォルダ（`NeutralDir`）隔離セッション・32k 文脈リプレイの安全不変条件を厳格に遵守します。「切断」ボタン押下時およびウィンドウクローズ時には、内蔵ランタイムのタスクと一時リソースが RAII により即座に安全回収されます。
- **外部ソケットへのフォールバック（手動入力）**: エンドポイント欄に既存の外部ソケットパス（例: `jcode serve` と Harness API ブリッジが提供する `%TEMP%\jcode-<USERNAME>\jcode-api.sock`）を手動入力して「接続」を押した場合は、内蔵ランタイムを起動せず、指定された外部ソケットへ接続します。
- **作業先の確定（第5段階）**: 監視パネルの「作業先」欄でフォルダの絶対パスと権限（`読み取り専用` / `読み書き`、既定値なし）を選んで **`確定`** すると、実在確認と正規化を行い計画データ（`計画（未実行）`）を表示します。現段階では作業セッションの起動やファイル編集は行わず、受付チャットの隔離・ツール無効も変わりません（[STAGE05.md](docs/vis-harness/STAGE05.md)）。

### 実会話 / Swarm 監視の前提（外部プロセス接続時）

`h2o-native` は上記のとおり**エンドポイント空欄で内蔵ランタイムを起動するため通常は外部プロセス不要**ですが、外部デーモンへ接続する場合や `h2o-monitor` を実行する場合は **Harness API ソケット**を用意します。

1. **外部プロセスとして daemon と Harness API ブリッジを起動する場合**:
   - `jcode serve`（例: `.\scripts\h2o.ps1 serve-daemon`）を起動したうえで、別プロセスでブリッジを起動します。
   - ブリッジは対応版 `jcode` の `jcode api-bridge`（注: 配布版 `jcode` v0.99.1 には未搭載）、または [`crates/jcode-harness-api-server`](crates/jcode-harness-api-server) からオンラインでビルドした `jcode-harness-api-bridge [API_SOCKET] [LEGACY_SOCKET]`（git 依存取得のためネットワークが必要。例: Git Bash で `export RUSTC_WRAPPER=; cargo build -p jcode-harness-api-server --bin jcode-harness-api-bridge`）を使います。
2. **接続先ソケットパス**:
   - Windows の既定 Harness API ソケットは **`%TEMP%\jcode-<USERNAME>\jcode-api.sock`**（PowerShell では `"$env:TEMP\jcode-$env:USERNAME\jcode-api.sock"`。環境変数 `JCODE_API_SOCKET` / `JCODE_RUNTIME_DIR` で上書き可）です。このパスを `h2o-native` のエンドポイント欄へ入力するか、`h2o-monitor` の `-Socket`（`--socket`）へ渡します。
3. **検証状態**: Windows 実環境での実モデルストリーミング（Real Chat Live LLM）および `h2o-monitor` の End-to-End 実行は **未検証（UNVERIFIED）** です（オフラインでの単一バイナリ起動、GUI サブシステム、Named Pipe IPC、切断・破棄ライフサイクル、および CLI usage 表示はすべて検証済み）。

その他の起動方法・CLI オプション・試験構成の詳細は [experiments/h2o-drift/README.md](experiments/h2o-drift/README.md) を参照してください。

## ドキュメント

- [REQUIREMENTS.md](docs/vis-harness/REQUIREMENTS.md): 要求仕様と受入条件
- [DESIGN.md](docs/vis-harness/DESIGN.md): 設計案
- [ACCEPTANCE_MATRIX.md](docs/vis-harness/ACCEPTANCE_MATRIX.md): 要求ごとに、どこまで検証したか
- [CONTROL_BOUNDARIES.md](docs/vis-harness/CONTROL_BOUNDARIES.md): ルーターと監視の設計境界
- 段階ごとの記録: [STAGE02](docs/vis-harness/STAGE02.md) / [STAGE03](docs/vis-harness/STAGE03.md) / [STAGE03_REINFORCEMENT](docs/vis-harness/STAGE03_REINFORCEMENT.md) / [STAGE04](docs/vis-harness/STAGE04.md) / [STAGE05](docs/vis-harness/STAGE05.md)
- UI: [NATIVE_UI.md](docs/vis-harness/NATIVE_UI.md) / [LIGHT_MODE.md](docs/vis-harness/LIGHT_MODE.md) / [REAL_CHAT.md](docs/vis-harness/REAL_CHAT.md)

## jcode 本体について

jcode 本体のインストール・使い方・ベンチマークは、upstream の
[README](https://github.com/1jehuang/jcode#readme) を参照してください。
このリポジトリは jcode とは別に管理しており、upstream 公式のものではありません。

## ライセンス

[MIT](LICENSE)。jcode 本体の著作権は Jeremy Huang に帰属します。

## 関連

- [H₂O Memory](https://github.com/nyattoh/h2o-memory)
- [SemEvolve](https://github.com/nyattoh/semevolve)
