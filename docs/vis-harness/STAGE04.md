# 第4段階：実Jev判定器とjcode Swarm接続（loopback試験まで）

## decision02

Kota承認：2026-11-11。提示した選択肢のうち「1と2」を選択。

1. 模擬判定を実Jev判定器へ置き換える。
2. 監視を実jcode Swarmへ接続する。

承認の範囲は実装とoffline／loopback試験だけである。実Jevへの送信と、LLM providerを呼ぶ可能性がある実jcode daemonの起動は未承認であり、実行していない。実行前に別途Kotaの承認を得る。その後2026-11-11、Kotaは合成データでの実Jev送信1回を承認し、親エージェントがPython参照clientで実行した（応答の形の節を参照）。実daemonの起動とRust transportでの実送信は引き続き未承認・未実行。
受付・bot UI、SemEvolve接続、router・model・effortの切替、H₂O元実装の移植は今回の範囲外。

## 実装した境界

### feature `jev`

`jev = ["sdk", "dep:ureq"]`。`jev` を有効にすると `sdk` も有効になる。
HTTP clientは `ureq =3.1.4`（`default-features = false`, `rustls`）。既にcache済みのcrateをofflineで解決した。ルートのlock・設定は変更していない。
`h2o-monitor` binaryは `required-features = ["sdk", "jev"]` で、`jev` 有効時だけbuildされる。

### `JevJudge`（`src/jev.rs`）

既存の `monitor::Judge` を実装する。監視・checkpoint・予算の処理は既存 `monitor.rs` のまま。

- 有効化：`JevConfig.enabled` の既定はfalse。trueでなければ構築を拒否する（`io::ErrorKind::InvalidInput`）。model名が空、deadlineが0、`min_probability` が[0,1]外の場合も拒否する。
- 鍵：環境変数 `TYPESAFE_API_KEY` だけを、各 `assess` の呼出時に読む。保持・記録・表示しない。HTTPの `Authorization: Bearer` headerだけで送り、request body・`Debug`・reason・journalに入れない。鍵が未設定または空白だけなら送信せず `Unavailable`。
- base URL：既定 `https://api.typesafe.ai`、送信先は `{base}/v1/systemone`。httpsは任意のhost、httpは `127.0.0.1`・`localhost`・`[::1]` だけを許可する。userinfo・query・fragment・空白・backslash・空host・不正port・scheme無し・https/http以外は拒否する。
- 質問：`goal_alignment` という `choice` 質問を1件だけ送る。選択肢は `aligned` / `misaligned` / `insufficient_evidence`。完了したツール観測が、固定した目的に寄与し、制約と所有範囲を破らないかを問う。model既定は `jev-latest`。
- 送信範囲（データ最小化）：`state` は次だけを含む。
  - 目的：purpose、constraints、success_conditions、owned_paths、unfinished。
  - 観測：ツール名、出力とエラーをそれぞれ `output_cap_bytes`（既定4096）で切り詰めた文字列。UTF-8境界で切り、`original_byte_length` と `truncated` を付ける。
  - 文脈：既定では原文IDと `byte_length` だけ。`include_context_bytes = true` の場合だけ原文を `encoding: "utf8"|"hex"` と `data` で無損失に送る。
  - run_id、branch_id、session ID、call ID、鍵は送らない。
- 判定の対応：`aligned`→Aligned、`misaligned`→Misaligned、`insufficient_evidence`→Unknown。選ばれた選択肢の確率が `min_probability`（既定0.6）未満ならUnknown。Unknownは既存契約どおり修正予算を消費しない。
- 故障の対応：transportのtimeout、またはdeadline経過後のtransport errorは `Timeout`。それ以外のtransport error、HTTP非2xx、redirect（追従しないため非2xxとなる）、鍵なしは `Unavailable`。JSONでない応答と想定外の形は `Malformed`。返却されたmodel IDが鍵を含む場合も `Malformed`。
- deadline：1回の呼出全体（接続から本文読取りまで）のhard deadline。既定10秒。応答本文は1 MiBまで。
- reason：`jev <label> p=<0.000> model=<返却されたmodel ID|unreported>`、返却されたときだけ ` confidence=<0.000>` と ` input_tokens=<n> output_tokens=<n>` を続ける。閾値は選ばれた選択肢の確率だけに適用し、confidenceは記録のみ。確率不足でUnknownにした場合もlabelと確率を残す。鍵は含めない。
- transport：`JevTransport` traitで分離する。本番は `UreqTransport`（rustls、proxy無効、redirect 0）。試験は同じ `UreqTransport` を127.0.0.1のstd::net HTTP serverへ向ける。

### 応答の形（2026-11-11に1回観測）

TypeSafeの資料に応答形式の記載はない。2026-11-11、Kotaの承認のもと親エージェントが合成データで1回だけ実Jevへ送信し、次の応答を得た（model `jev-1.14.0`、`target/swarm-run/live/response.json`）。この1件を試験 `observed_live_response_is_judged` に原文のまま固定した。

```json
{"model":"jev-1.14.0","answers":{"goal_alignment":{"type":"choice","choice":"aligned","confidence":0.62,"probabilities":{"aligned":0.75,"misaligned":0.04,"insufficient_evidence":0.21}}},"usage":{"input_tokens":573,"output_tokens":47}}
```

許可するフィールドはこれだけで、すべての階層で未知のキーは `Malformed` とし、推測で解釈しない。

- 最上位：`model`（任意、文字列）、`answers`（必須）、`usage`（任意）。任意のフィールドは省略できるが、明示的な `null` は `Malformed`（`model`・`usage`・`confidence` とも）。`error`・`status`・`id` など他のキーは `Malformed`。
- `usage`：`input_tokens`・`output_tokens`（ともに必須、非負整数）。他のキーは `Malformed`。
- `answers`：`goal_alignment` だけ。
- 回答：`type`（`"choice"`）、`choice`（3つのlabelのいずれか）、`confidence`（任意、有限の[0,1]の数値）、`probabilities`（必須）。

probabilitiesは `aligned`・`misaligned`・`insufficient_evidence` の3つを各1回だけ持ち（欠落・重複・他のキーは `Malformed`）、値は有限の[0,1]の数値。選ばれた選択肢が最大確率を持たない場合も `Malformed`。観測は1件だけであり、modelの版が変わって形が変われば全件 `Malformed`（Unknown扱いで保留）になる。

### `run` loop（`src/run.rs`）

`run(client, spec, sources, journal, &mut judge, poll) -> io::Result<(RunEnd, checkpoint数)>`。既存の `SdkMonitor::attach` と `poll` を繰り返し、停止条件だけを加える。

- `NeedsReconciliation`：制御要求の結果が不明・失敗（接続断の後を含む）。再実行前に人が照合する。照合待ちは他の終了条件より先に判定する。
- `Stopped`：journalの最後の記録が `StopBranch`（cancelが受理済み）。停止済みjournalを再openした場合もこれを返し、新しい要求を出さない。
- `Disconnected`：SDKのtransportが閉じ、照合待ちの制御要求がない。

接続断のunknown checkpointは既存契約どおり一度だけ記録する。そのHoldのcancelは閉じたclientで失敗するため、journalは照合待ちになり、`run` は `NeedsReconciliation` を返す。現行の監視では接続断は通常この経路になり、`Disconnected` は照合待ちのない接続断の場合だけ返る。

### `h2o-monitor` CLI（`src/bin/h2o-monitor.rs`）

```text
h2o-monitor --enable-jev --socket <path> --spec <json> --journal <dir> --sources <dir>
            --owner <session> --worker <session> [--jev-base-url <url>] [--min-probability <p>]
            [--deadline-ms <ms>] [--output-cap-bytes <n>] [--include-context-bytes] [--poll-ms <ms>]
```

- `--enable-jev` を最初に確認し、無ければspecを読む前・接続前に終了する。未知の引数・重複した引数はエラー。
- `--spec` は `BranchSpec` のJSON。`--owner`/`--worker` はspecの `owner_session_id`/`session_id` と一致しなければ拒否する（上書きしない）。
- `--sources` は既存のディレクトリ（`Memory`）。`--journal` は無ければ作成し、run/branchごとに専用とする。
- 省略時の値は `JevConfig` の既定。`--poll-ms` の既定は200。
- 接続は `JcodeClient::connect`（`socket_path` 指定、`ensure_runtime: false`、`client_name: "h2o-monitor"`）。daemonやruntimeを起動しない。
- 出力：stdoutへ `Stopped|Disconnected|NeedsReconciliation checkpoints=<n>`。
- exit code：0＝Stopped／Disconnected（照合待ちなし）、3＝NeedsReconciliation（接続断でcancelが届かなかった場合を含む）、2＝引数・spec・設定・接続・監視のエラー（stderrに `h2o-monitor: <message>` とusage）。鍵は出力しない。

## 検証

リポジトリのルートから、専用のPowerShellプロセス内で実行する。

```powershell
$env:RUSTC_WRAPPER=''
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --no-default-features
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features sdk
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features jev
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features "sdk jev"
```

2026-11-11、Windowsでの最新結果（いずれも失敗0、exit 0）：

| 構成 | 成功 | ignored |
|---|---|---|
| `--no-default-features` | 24 | 3 |
| 既定 | 24 | 3 |
| `--features sdk` | 52 | 4 |
| `--features jev` | 71 | 5 |
| `--features "sdk jev"` | 71 | 5 |

ignoredは既存の明示実行用fixture・長時間試験と、`jev` 有効時の `tests/jev_live.rs` 1件（実Jev呼出、既定で実行しない）。
追加前の `--features sdk` は47件（ignored 4）。`sdk` の増分5件は `run_loop`、`jev` の増分24件は `jev_judge` 15件、`run_loop` 6件、`h2o_monitor_bin` 3件。

追加した試験：

- `tests/jev_judge.rs`（15件）：3つのlabelの対応とrequestの形（`POST /v1/systemone`、server側でBearerを確認、bodyに鍵・call IDなし）、低確率→Unknown、HTTP 500→Unavailable、非JSON→Malformed、想定外の形11種→Malformed、probabilitiesの欠落・重複・負値→Malformed、観測した実応答の原文→Aligned（reasonにmodel・confidence・tokens）、各階層の未知キー（最上位の `error`・`status`・`id`、`usage` の余分なキー、回答内の余分なフィールド）、不正なconfidence、`model`・`usage`・`confidence` の明示的な `null`→Malformed、停止したserverへの呼出がdeadline 300msで `Timeout`（経過2.3秒未満）、鍵なし・空→Unavailableかつ接続なし、無効設定・非loopback http・userinfo等の拒否、文脈原文はopt-in時だけ送信、UTF-8境界での切詰め、鍵が `Debug`・reason・request body・監視journalの全ファイルに現れないこと。
- `tests/run_loop.rs`（6件、うち1件は `jev` 有効時のみ）：接続断は待ち時間ではなく、peerが最初のAttachSessionに応答した直後に閉じる。メモリ内の模擬SDK peerで、予算超過のMisaligned→SoftInterrupt後にStopBranch→Cancelで `Stopped`、接続断で一度だけunknownを記録して `NeedsReconciliation`、制御要求の失敗で `NeedsReconciliation`、別sessionのイベント無視、停止済みjournalの再open、`JevJudge` と127.0.0.1のstubでのrun。送信した要求はHello/AttachSession/Cancel/SoftInterruptだけであることを確認する。
- `tests/h2o_monitor_bin.rs`（3件）：`--enable-jev` なし、owner不一致、存在しないsocketでいずれも非0終了し、接続前に止まる・runtimeを起動しない（10秒未満）・journalを作らないことを確認する。子プロセスからは `TYPESAFE_API_KEY` を除去している。

試験の通信は127.0.0.1だけ。実鍵は読まず、試験はlookupを差し替えて偽の鍵を使う。
RED/GREENの記録（実行コマンド・exit code）は作業証拠 `target/swarm-run/evidence-T1.md`・`evidence-T2.md`・`evidence-fix-r1.md`・`evidence-fix-r2.md`・`evidence-fix-r3.md`・`evidence-fix-r4.md`（独立レビュー指摘の修正）にある。`jev_judge` と `run_loop` は実装前の失敗（exit 101、未定義module）を記録した。`h2o_monitor_bin` はbinary実装後に書いたため、RED記録はない。
`cargo clippy` は未実行（toolchainに未導入）。

## 実Jevへの単発確認（`tests/jev_live.rs`、未実行）

Rustの `UreqTransport`（https／rustls）で実Jevへ1回送る `#[ignore]` 試験。`H2O_JEV_LIVE=1` のときだけ送信し、2026-11-11の観測と同じ合成入力を使い、結果が `Ok`（signalは問わない）であることを確認する。`Unavailable`・`Timeout`・`Malformed` のいずれも失敗とする。実行には別途Kotaの承認が必要。鍵は運用者のshellだけで設定する。

```powershell
$env:RUSTC_WRAPPER=''
$env:H2O_JEV_LIVE='1'
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features jev --test jev_live -- --ignored --nocapture
```

## 手動の実接続手順（未実行）

**未実行。実Jev送信と実daemonを伴うため、実行前に別途Kotaの承認が必要。**

前提：

1. 運用者が起動済みのjcode daemonとharness API bridgeがあり、APIのsocket pathが分かっている（`h2o-monitor` は起動しない）。
2. owner sessionと、その直接の子（`parent_session_id` がowner）であるworker sessionがある。
3. `spec.json` はそのIDを持つ `BranchSpec`。run_id・branch_id・purpose・success_conditionsは空にしない。`context_ids` は `--sources` に存在しなければUnknownになる。
4. run/branchごとに新しい専用journal dir。
5. `TYPESAFE_API_KEY` は運用者のshellだけで設定し、ファイルに書かない。Jevへは目的、ツール名、切り詰めた出力・エラー、文脈のIDと長さが送られる。

```powershell
$env:RUSTC_WRAPPER=''
cargo build --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features "sdk jev" --bin h2o-monitor
target/h2o-drift/debug/h2o-monitor.exe --enable-jev `
  --socket <jcode API socket path> `
  --spec <path>/spec.json --journal <path>/journal-<run>-<branch> --sources <path>/sources `
  --owner <owner_session_id> --worker <worker_session_id> --poll-ms 200
```

exit 3（NeedsReconciliation）の場合は、再実行前にjournalの `receipt-*` 記録とworkerの状態を確認する。同じjournalで再実行しても設計どおりNeedsReconciliationを返す。
binaryは手動実行していない（試験からの起動だけ）。

## 未検証・unknown

- 実Jevの応答形式は2026-11-11に1件（`jev-1.14.0`、合成入力、`choice=aligned`）だけ観測した。他のlabel・低確率・エラー時の実応答、他のmodel版での形は未観測。
- その実送信は、JevJudgeが組み立てるものと同一のrequest bodyを、Pythonの参照client（`jev_call.py`）で送ったもの。Rustの `UreqTransport` のhttps／rustls経路そのものは実行していない（compileのみ、`tests/jev_live.rs` は未実行）。試験はloopbackのhttpだけ。
- proxyの環境変数は無視する。redirectは追従しない。社内proxy等の環境での到達性はunknown。
- 実daemonでのイベント配信、Windowsのsocketでの `attach_session`、接続断の検出は、メモリ内peerと存在しないsocketへの接続失敗でしか確認していない。
- 文脈原文のbyteは、既存のMonitorが従来どおりローカルjournalの `JudgementInput` に保存する。最小化したのはJevへの送信だけ。
- 接続断の後は `NeedsReconciliation`（exit 3）になる。照合待ちのない `Disconnected`（exit 0）は現行の監視では到達しないと考えられるが、試験では確認していない。
- 制御要求の失敗後に待っていたツールイベントは判定しない（loopが先に終わる。意図した動作）。
- Jev呼出のレイテンシ、トークン、費用はunknown。
- `cargo clippy` は未実行（未導入）。
- 別vendorによる独立code review（受入条件A6）は、この文書の作成時点で未完了。
