# 制御境界とルーティングの設計入力

対象：decision01の第2〜3段階。Kotaの設計メモ（2026-11-10 21:17 JST）を、確認できたコードと未実装の案に分けて整理する。
受付・bot設定・通信は第4段階のため着手しない。以下の将来案は実装済みAPIや追加実行の承認を意味しない。

## 独立させる4つの機能

1. マルチエージェント化は、作業を誰が所有し、何人がどの範囲を実行するかを扱う。1 workerでも成立し、モデル変更やバックグラウンド化を必須にしない。
2. モデル選択は、利用可能なprovider・model・認証経路の候補から担当を選ぶ。worker数の増減や監視結果から独立に記録する。
3. effort調整は、選んだモデルが受け付ける推論設定を扱う。model名から能力を推測せず、要求値と実効値と未確認を区別する。
4. バックグラウンド実行は、実行の寿命・所有者・再開・通知を扱う。画面非表示、別プロセス、別モデル、追加workerとは同義にしない。

目的監視はこの4つを上書きする権限ではない。元の目的・制約に対して観測がどう関係するかを記録し、限定した制御要求を出す。

## 現行コードから確認できた境界

- [AgentsConfig](../../crates/jcode-config-types/src/lib.rs)：`swarm_model`、`swarm_model_selection`、`swarm_model_allowlist`、`swarm_effort`、`swarm_root_effort`を別項目で保持する。モデルoverrideはopt-inと完全一致allowlistで検査する。bare modelの許可を別providerの認証経路の許可と読み替えない。
- [Swarmツール](../../crates/jcode-app-core/src/tool/communicate.rs)：worker生成時のmodelは設定の制約を受ける。モデルからのeffort指定は転送せず `None` とする。[schema](../../crates/jcode-app-core/src/tool/swarm_model_schema.rs)もmodel選択を設定に合わせて制限する。
- [低水準プロトコル](../../crates/jcode-protocol/src/wire.rs)：`CommSpawn`にはmodelとeffortの任意引数がある。これを受ける[サーバー](../../crates/jcode-app-core/src/server/comm_session.rs)はrequested effortを取り込む。
- [spawn policy](../../crates/jcode-app-core/src/server/spawn_policy.rs)：明示effort、設定の `swarm_effort`、既定lowの順で解決する。この関数自体が各モデルの対応effortを検証するとは確認できない。
- [spawn selection](../../crates/jcode-app-core/src/server/spawn_model_selection.rs)：明示model、session/configの既定、coordinatorからの継承を解決し、明示された認証経路を保持する。認証経路とmodelを一つの曖昧な名前へ潰さない。
- [公開harness API](../../crates/jcode-harness-api/src/requests.rs)と[SDK](../../crates/jcode-sdk/src/client.rs)は低水準daemonプロトコル全体と同一ではない。`CommSpawn`の引数がそのまま公開 `JcodeClient` の生成APIで使えるとは扱わない。
- [runtime commands](../../crates/jcode-app-core/src/server/runtime_commands.rs)にはSwarm経由のsubagent生成経路もあるが、この経路はモデル向けSwarmツールの制約を通る。今回実行していない。
- [provider設定](../../crates/jcode-config-types/src/lib.rs)と[failover分類](../../crates/jcode-provider-core/src/failover.rs)は通信・認証・quota等を扱う。目的逸脱の判定ではない。分類関数がfailover候補を返すことと、実際の送信・課金・成功は別の事実である。

従って、fast/deepのようにprovider+model+effortを束ねる設計では、まず利用するAPI境界と対応能力を確認する。低水準引数の存在から、モデル向けツールや全providerでの個別effort調整を実証済みにしない。
上記はコード読取りの結果であり、実daemonや実providerに対する動作試験ではない。

## 第3段階で接続した監視

`BranchSpec`はrun/branch/owner/session、目的、制約、成功条件、所有範囲、未完了事項、選択原文ID、修正予算を固定する。
監視は完了済み `ToolDone` だけを観測し、モデルの内部推論を証拠として使わない。

H₂Oからは選択IDの原文だけを取り出す。別workerの履歴全体を親の文脈へ継続的に追加しない。
この版は必要なIDを呼出側が指定する方式であり、要約・索引・自動選択・相遷移を実装したという意味ではない。

`Continue`は追加制御なし、`Correct`は原文復元を含む再計画要求、`Hold`と`StopBranch`は対象turnの取消しへ対応する。
監視adapterはmodel・provider・effortの切替要求、追加worker生成、セッション削除を発行しない。
unknownは予算を消費せず、制御要求の応答が不明なら自動再送しない。
元の目的を書き換えて「整合した」状態を作ることはせず、変更された契約を同じjournalへ読み込もうとすると拒否する。

制御要求の受付、workerの理解、実際の再計画、成果の受入は別々に観測する必要がある。
今回のSDK模擬peerは前者までを試験する。workerの理解や実成果の改善はunknown。

## 将来のルーターの入力と出力案

入力候補は、依頼の目的・成功条件、作業の種類、所有範囲、利用可能経路のsnapshot、許可された経路、測定可能な予算と時間制約とする。
経路の取得元・取得時刻・有効性が不明なら、それをunknownとして候補に残すか除外する判断を明示する。offline fallbackを現行の完全な一覧としない。
APIキーの存在や過去の接続成功だけで、その経路が現在実行可能と結論しない。

出力候補は、選んだ経路、要求effort、適用できた実効effort、選択根拠、代替候補、未確認事項、方策の版とする。
fast/deepは方策上の束名であり、実在するmodel IDの代わりに生成APIへ渡さない。
束の解決に失敗しても、未知のproviderや高額な候補へ勝手に切り替えない。

既存config・カタログ・allowlist・認証経路を尊重してSwarmへ渡し、独自schedulerで並行して同じworkerを所有しない。
実行中の逸脱だけで経路を変更せず、まず再計画と必要な文脈復元を試す。
経路変更が必要だという提案を監視が出せるようにする場合も、実際に変更する判断と権限はルーター／利用者側に残す。

## 判断の良否を評価する案

担当選択の質を、workerが起動したか、監視がalignedを返したか、試験数が増えたかだけで評価しない。
依頼ごとに次の事実を結び付けて記録する案とする。

- 選択時の入力・経路候補snapshot・選択理由・方策版。
- 実際に使った経路とeffort。取得不能ならunknown。
- 依頼の受入条件に対する独立した検証と、その根拠となる成果物。
- 再計画・文脈復元・停止・再試行・実行基盤故障の有無。
- 実測できたトークン、時間、費用。計測がなければnull/unknown。
- 選択外の候補を実行していなければ、その候補より良かったかはunknown。

ルーター自身の選択理由と、監視自身の整合判定を、そのまま学習の正解へ使わない。
比較実験では同じ依頼・受入条件・権限・利用可能な情報をそろえ、監視や文脈復元の有無とモデル選択の効果を分ける。
モデル切替、effort変更、worker数、バックグラウンド化を同時に変えて、どれが寄与したかを断定しない。
既に使ったfixtureへの調整と、未使用課題での検証を分ける。

SemEvolveへ渡す評価信号もこれらの根拠と方策版に結び付ける案とし、低信頼・未測定・利用不能を正解として埋めない。
既存SemEvolve実験・DB・承認状態の更新は、この設計整理に含めない。

## 混同しない故障と判断

- workerが目的から外れた観測：再計画・文脈復元・修正予算の対象。
- 判定器が使えない／証拠が足りない：unknownで保留。高価なworkerへ昇格する根拠にしない。
- provider通信・認証・quota故障：実行基盤の故障。別providerへの送信は既存の許可とfailover契約を別途確認する。
- SDK制御の応答不明：適用済みか分からないため照合待ち。同じ副作用を自動再送しない。
- 原文やjournalの欠損：原文復元・予算復元を成功扱いせず、元データと残存記録を保持する。

これらを全て「逸脱」にまとめて予算を消費したり、全て「通信失敗」として成果を捨てたりしない。

## 次の統合確認に必要な証拠

第4段階以降の着手承認後、受付の対象確定、権限制御、実Swarmの実行寿命とauto-pokeを含む保留／停止、複数clientとの関係を検証する必要がある。
公開SDKで利用できるworker生成・effort指定の境界、実効経路の読取り、文脈復元要求の受領と理解も確認する。
実Jevは送信範囲と明示的有効化の承認後だけに限定する。
ここに挙げた将来確認のためのdaemon起動・provider接続・受付実装は、今回行っていない。
