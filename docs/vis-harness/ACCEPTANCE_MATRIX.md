# 要求と検証境界

decision01の第2〜3段階と追加補強の対応表。合成試験で確認した契約と、実接続・実利用の受入を区別する。
要求自体は [REQUIREMENTS.md](REQUIREMENTS.md)、設計は [DESIGN.md](DESIGN.md)、追加の仮定と記録は [STAGE03_REINFORCEMENT.md](STAGE03_REINFORCEMENT.md) を参照する。

| 要求 | 現在の成果 | 根拠となる試験・資料 | 残る境界 |
|---|---|---|---|
| 再起動後の原文復元 | 別プロセスからbyte単位で取得 | memory/persistence_testsの正常終了writer→別reader、kill後→別reader | 電源断、OSクラッシュ、破損・改ざん検出、削除・訂正・同意・アクセス制御は未検証／未実装 |
| ID衝突を報告 | 公開時の上書きを拒否 | 逐次の別handle、同時の2プロセス、公開APIの3 writer競合 | 異なるFS間のID正規化や大文字小文字の統一は保証しない |
| 中断中に未完成原文を読ませない | sync後に同じFSのhard linkで公開 | Created/Written/Synced/Publishedでの観測・kill、並行reader | hard link非対応FSはエラー。全FSでの動作保証ではない |
| 復元失敗を報告 | 原文不在・I/Oエラーを伝播 | memory_contract、欠損原文による監視Hold | 内容の改ざん・破損を検知するdigestは未実装 |
| 枝の進行・修正・上限停止 | 純粋状態機械とSDK adapter | drift_contract、drift_state_space、monitor_contract、sdk_monitor | 実workerの理解・再計画・成果改善は未観測 |
| 判定器故障時に保留 | 模擬timeout/unavailable/malformedと証拠不足をunknown化 | monitor_contract、sdk_monitorのcancel要求 | 実Jevは未接続。同期Judgeの実時間制限は実装側の契約 |
| 故障で枝や履歴を削除しない | checkpoint先行保存、対象turn取消し | control直前の記録確認、応答喪失、実プロセス再起動試験 | 実daemonのauto-pokeや他clientによる再実行を阻止する権限機構ではない |
| 元の目的・成果・根拠を保持 | 契約・観測・選択原文・判定・要求をjournalへ保持 | 契約変更拒否、予算復元、欠番・不正JSON・孤立応答・ID矛盾の反例 | 完全な破損検出、監査・訂正・復旧UI、成果の独立検証は残る |
| 必要な情報だけworkerへ渡す | 呼出側が選んだcontext IDだけ取得 | unrelated原文がJudgeと再計画要求へ入らない試験 | 自動検索、要約、索引、送信量制限、元H₂Oの相表現は未実装 |
| SDK／Swarmの所有関係を守る | 指定ownerの直接の子のみを確認し、要求直前にも再確認 | owner不一致・変更の試験 | 確認と制御の原子性、深い子孫の権限、実daemon側の強制は未検証 |
| 受付で作業dir入力不要 | 設計のみ | CreateSessionOptionsの読取り、初期設計 | 第4段階のため受付・対象確定・権限制御を実装していない |
| botの設定追加とadapter能力確認 | 未着手 | 要求仕様と段階計画 | 実bot起動・接続・権限はunknown |
| 2 botの版付き通信・受領・理解確認 | 未着手 | 要求仕様と段階計画 | SemEvolve通信・方策更新は未接続。既存DBや研究状態は未変更 |
| 実Jevの明示的有効化と送信制御 | 未着手、実通信なし | 実装はJudge traitとローカルmockのみ | 別途Kota承認が必要 |
| 計測 | 論理常駐payload量と試験件数・時間・入力byte数を区別 | resident_bytes、公開API試験の統計 | RSS、ピークRSS、OS物理I/O、トークン、費用、電力はunknown |
| 担当選択の判断と評価改善 | 設計入力を整理 | CONTROL_BOUNDARIES.md、既存routingコード読取り | 新router、学習、実モデル比較、改善効果は未実装・未検証 |

## 証拠の読み方

純粋状態機械の全列試験は枝制御の契約を調べるもので、逸脱検出の精度を調べていない。
メモリ内peerと実SDKを使う試験はframing・購読・要求の契約を調べるもので、実daemonの挙動を調べていない。
実子プロセスのkill試験はファイル公開・journal復元・制御順序を調べるもので、電源断や実外部副作用を調べていない。
長時間試験は入力と実プロセスの割込み順を変えて反例を探すもので、有限回の成功を耐障害性の完全な証明へ置き換えない。

試験の入力、mockの回答、制御markerは合成データである。異なる名前の試験を同じ成果の独立した実利用評価として数えない。
外部接続、ユーザーの非公開原文、実Jev、課金、公開・リリースによる受入は今回の証拠に含まれない。

## 第4段階の前に残す事項

未検証の境界を解消したという前提で受付やworkerを自動実行させない。
第4段階着手後も、対象確定・権限・実daemonでの保留と停止を具体的な受入条件にし、モデル選択・effort・実行寿命・目的監視を分けて観測する。
これは次段階のための設計上の記録であり、今回の実装範囲を広げる承認ではない。
