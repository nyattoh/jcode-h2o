# 第2段階：原文の公開とプロセス中断

Kota承認：2026-11-10 21:17 JST。独立crate内の作業。共通設定とscriptsは変更しない。

## 元実装の保存契約

2026-11-10、要求仕様が参照する `experiment/physical-memory-energy-v0.3` の次の公開ソースをGitHub経由で読んだ。元リポジトリへの書込みや試験実行はしていない。

- [測定要件](https://github.com/nyattoh/h2o-memory/blob/experiment/physical-memory-energy-v0.3/doc/experiment-v0.3.html)：論理常駐量、物理RSS、I/O、時間、電力を区別する。
- [cold_archive.ts](https://github.com/nyattoh/h2o-memory/blob/experiment/physical-memory-energy-v0.3/src/lib/phase_memory/cold_archive.ts)（blob `5d389902331ebceb05870f325b70546c30ee0b64`）：put/get/delete/has/statsのbyte列保存API。
- [cold_archive.node.ts](https://github.com/nyattoh/h2o-memory/blob/experiment/physical-memory-energy-v0.3/src/lib/phase_memory/cold_archive.node.ts)（blob `426f90241286fa9c344dcf957d6ed65a69ec0580`）：キー由来のcapsule名、一時ファイル書込み後rename。既存キーの置換を拒否せず、fsync・競合制御は実装されていない。
- [tiered_memory.ts](https://github.com/nyattoh/h2o-memory/blob/experiment/physical-memory-energy-v0.3/src/lib/phase_memory/tiered_memory.ts)（blob `60de5335165f8207a67c42c56ccd108437f3ede1`）：常駐表現からcold capsuleを分離しhandleで復元。検索対象を選んで復元し、派生表現・索引・出典・訂正・転送記録も扱う。
- [tiered_memory.test.ts](https://github.com/nyattoh/h2o-memory/blob/experiment/physical-memory-energy-v0.3/src/lib/phase_memory/tiered_memory.test.ts)（blob `f5170c6d856346957ee8bca961e368d350a25dac`）：adapter・可逆復元・選択的復元の試験を読んだ。今回の実行結果ではない。

このcrateは既存要求どおり原文byte列を保持し、同一IDの上書きを拒否する。元実装の相表現・圧縮・索引・訂正・出典を移植したものではない。Ice/Waterは完全なRAMコピー、Vapourはdisk原文から復元する従来の試作品の意味を維持する。

## 実装と受入条件

1. 保存先と同じディレクトリに、一意名で排他的に一時ファイルを作る。
2. byte列を書き込み、ファイルをsyncして閉じる。
3. hard linkでID名を公開する。既存IDは置き換えず、競合はAlreadyExistsとする。
4. 自分の一時名だけを除去する。中断・除去失敗の残存物は `pending_files()` で列挙する。

一時名は通常IDの文法に入らず、get/transitionで読めない。残存物を完成原文へ自動昇格しない。生存中のwriterの可能性があるため、自動削除もしない。旧配置の正本はそのまま読める。

hard link非対応FSではエラーとし、上書きrenameや部分copyに退行しない。公開直後にwriterの応答が失われた場合、呼出側には結果が不明となるが、再openして原文を取得でき、再挿入は衝突する。

## 検証記録

再現コマンドは `experiments/h2o-drift/README.md` と同じ。専用PowerShellプロセス内だけでwrapperを無効化した。

- RED：`readers_never_observe_uncommitted_bytes`。旧保存方式に観測点を加え、公開前に空byte列が読めて失敗（exit 1、1件失敗）。
- GREEN：Windows上で全20件成功、失敗0、exit 0。ignored 1件は親試験から明示起動する子プロセス用fixture。
- 作成中・部分書込み後・sync後は別readerからNotFound、公開後は全byte一致。
- 公開前の各観測点でエラーを注入し、未完成原文がなく既存原文が保持される。
- 実子プロセス2つを同期して同一IDへ書き込み、成功1・衝突1、勝者の全byte一致を確認。
- 作成直後・64 KiB書込み後・sync後・公開後で実子プロセスをkillし、未公開または完全な正本のみとなることを確認。
- kill後に別の新規子プロセスから復元。通常終了writerから別readerへの復元も確認。
- 残存する一時ファイルが正常な再挿入を妨げず、再openが勝手に削除しない。

## 限界

検証対象はこのWindowsホストの一時ディレクトリ上のFS。電源断、OSクラッシュ、ディレクトリエントリの永続化、ネットワークFS、改ざん・破損検出、アクセス制御は未検証。容量枯渇や物理I/O故障を実機で発生させたものではなく、エラー注入である。RSS、エネルギー、実環境の効果はunknown。

独立workspaceを維持する。第3段階ではSDK公開境界へ接続し、並列化・モデル選択・effort調整・バックグラウンド実行と監視を分離する。逸脱だけでモデルを変更せず、まず再計画・文脈復元を要求する。
