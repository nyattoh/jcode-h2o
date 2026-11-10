# 第3段階：SDK／Swarm境界の模擬監視

Kota承認：2026-11-10 21:17 JST。第4段階へ進む前に停止する。

## 接続した境界

独立crateの任意feature `sdk` から、既存 `jcode-sdk` をpath依存で使う。
ルートworkspaceへの追加、SDK・Swarm本体・共通設定・scriptsの変更は行わない。

- `SdkMonitor::attach`：イベント購読を先に作り、SDKの `attach_session` でworkerを確認する。
- `SessionInfo.parent_session_id`：指定ownerの直接の子だけを対象にする。制御要求直前にも再確認する。
- `ApiEvent::ToolDone`：完了済みツールのID・名称・出力・エラーを観測する。別session、推論表示、未完了の引数断片は判定に使わない。
- `Judge`：目的・制約・成功条件・所有範囲・未完了事項、観測、明示選択した原文だけを受け取る。実装した試験はローカルの模擬判定器だけ。
- `Correct`：H₂Oから選択した原文を復元し、元の目的と制約を含めた再計画要求をSDKの `soft_interrupt` へ送る。
- `Hold` / `StopBranch`：対象workerのturnだけを `cancel` する。セッション・成果・履歴を削除しない。
- `Continue`：制御要求を発行しない。成果の統合・検証が成功したという意味ではない。

判定器のtimeout・利用不能・不正応答、根拠のない回答、原文の復元失敗、観測不足はunknownとし、修正予算を消費しない。
同期traitの判定器には実行時間を自ら制限する契約を置く。実ネットワークのtimeout機構は未実装・未試験。
SDKの待受けtimeoutは単なるidleとして扱う。接続断は一度だけunknownを記録する。

## 記録と再開

branch/runごとに専用journalを置き、初期契約、各checkpoint、SDK制御要求の応答記録を第2段階のimmutable保存で保持する。
checkpointを先に保存し、その後に制御要求を出す。制御の応答が不明・失敗の場合は `needs_reconciliation()` をtrueにし、追加制御を拒否する。
再open時は予算と停止状態を復元し、同じcall IDを再評価せず、制御要求を自動再送しない。
欠けた応答記録、不正JSON、checkpointの欠番、契約の変更を黙って正常扱いしない。

SDKのOkは要求の受付応答であり、workerが要求を理解して再計画した証拠ではない。
不明な制御結果の照合・再開UIは未実装。journalを勝手に書き換えて再開する運用は想定しない。
監視インスタンスの再openは同一プロセス内で試験しており、監視プロセスのkill中断は未試験。

## Kota設計メモとの関係

並列化、モデルの自動選択、effortの自動調整、バックグラウンド実行は独立した機能として扱う。
今回の監視APIにprovider/model/effortの変更権限はなく、逸脱に応じて高価なモデルへ自動昇格しない。
既存Swarmの実行・ルーティング機構は置換しない。新規workerや独自schedulerは作らない。

親へ全履歴を積み増す代わりに、必要な `context_ids` だけをdiskから取得し、workerへ再計画の文脈として渡す。
目的・判断・未完了事項と観測結果はjournalに残し、記録は必要なcheckpointだけを明示的に読む。

方向の相違は導入していないが、次の点は未到達である。

- 誰に任せるかの判断、その評価・改善、Hermesのmodel_mapを参考にした束ね方は未実装。
- 確認した `AgentsConfig` はモデル選択のallowlistとworkerの `swarm_effort` を分けており、workerごとに任意のeffortを束ねられるとは未確認。自動effort調整が既にあるとは扱わない。
- H₂O元実装の派生表現・索引・自動相遷移を移植していない。選択する原文IDは呼出側が指定する。
- SemEvolveが評価結果から方策を改善する接続は未実装。既存研究DBや承認状態は変更していない。

## 検証

リポジトリrootから専用PowerShellプロセスで実行する。

```powershell
$env:RUSTC_WRAPPER=''
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features sdk
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --no-default-features
```

2026-11-10、Windows上でSDK有効34件成功、SDK無効20件成功、いずれも失敗0・exit 0。
各構成のignored 1件は親から明示実行する子プロセスfixtureであり、受入試験の省略ではない。
SDK依存の初回offline解決は未取得の公開パッケージで失敗したため、テストプロセスで依存を取得した後にofflineで全試験を実行した。ルートのlockや設定は変更していない。

監視契約10件では進行・修正・復帰・上限停止、判定器故障による保留、選択的な文脈復元、重複除外、所有者不一致、予算の再open復元、応答不明・履歴破損・競合を確認した。
SDK境界4件は実 `JcodeClient::connect_with`、framing、reply相関、イベント購読を使い、相手だけをメモリ内の模擬peerにした。
観測した要求はHello/AttachSession/SoftInterrupt/Cancelだけであり、モデル切替・worker生成・セッション削除要求を発行しない。
所有者変更、制御失敗、接続断、idle待受けを区別した。
第3段階のREDは、空白だけの出力と空のエラーを観測不足として保留する追加回帰試験で採った。修正前は判定器を呼び出して失敗（1件失敗、exit 1）、修正後は判定器を呼ばずHoldとなり、上記全試験が成功した。

## 未検証の境界と停止点

実daemon・実Swarm scheduler・実bot adapter・実Jevは未接続。API境界の契約試験通過であり、実利用体験の証拠ではない。
監視はツール完了後の協調制御であり、既に起きた副作用を取り消したり、別clientやdaemonの自動再起動を権限境界として防止する機能ではない。
保留・停止したrunに対して、このadapterは新規turnを送らない。実daemonのauto-poke等を含む再実行抑止は統合試験で別途確認が必要。
原文は非機密の合成fixtureだけを使用。実Jev通信、鍵の読取り、公開、有料サービス利用は行っていない。
RSS、電力、費用、実環境での目的逸脱検出精度・改善効果はunknown。

第4段階（受付・bot設定・通信）は着手せず、ぐろっくん宛て報告後にKotaの判断を待つ。
