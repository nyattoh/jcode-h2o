# Rust受付の実SDKチャット接続

2026-11-12。デモのみだったRustネイティブ受付に、既存jcode harnessへの明示接続とSDK応答表示を追加した。コード・制御peer・ヘッドレスUIの受入範囲であり、実daemon/provider/modelとの会話成功、実ウィンドウ・GPU・OS IMEは未観測。元のREQUIREMENTS全体が完成したという意味ではない。

## 最新版を開く

このcheckoutのビルド済み実行ファイル:

```powershell
& 'D:\develop\works\jcode-h2o\target\h2o-real-chat-ui\release\h2o-native.exe'
```

初期状態はライト・実会話モード・未接続・endpoint空欄。アプリを開くだけでは接続も送信も行わない。ダークはヘッダーで選べる。デモは明示的に選択する別モード。以前のdark-only/demoバイナリとは区別する。

ソースからの再ビルドはリポジトリルートのGit Bashで:

```bash
export RUSTC_WRAPPER=
cargo build --offline --locked --release --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-real-chat-ui --features native-ui --bin h2o-native
```

依存キャッシュと既存の独立crateのCargo.lockが必要。root/shared設定は変更しない。

## 実際に会話する前の条件

1. 既存の正常なjcode daemon/API bridgeと認証済みproviderが必要。このアプリは起動・ログイン・鍵取得を行わない。
2. 接続先は、現行source相当の`create_session.system_prompt`全文置換と`system_reminder`転送を尊重するruntimeが対応条件。両者には独立したcapability/ACKがなく、任意の旧版を保証しない。
3. 利用者が既存API endpointの正確なパスを確認して入力し「接続」を押す。UIはユーザー名や環境からパスを推測・自動入力しない。作業プロジェクトの入力は不要。
4. `session_tools`、`configure_tools(enabled=Some([]), custom=[])`のACK、有効ツール一覧が空であることを確認できなければ送信可能にならない。
5. 入力・送信範囲、provider/model、保存可能性と費用を確認し、別途実サービス送信の承認を得てから送る。今回の自動検証では実サービスを呼び出していない。「接続設定済み」はprovider応答成功ではなく、初回実応答は受信するまで未確認と表示する。

## 接続と会話の境界

`ensure_runtime:false`の実JcodeClientを使用し、既存workerをattachしない。新規の空の作業ディレクトリと固定promptで隔離sessionを作り、全ツールを無効にする。auto-approvalはない。ツール・権限要求が来たら失敗閉鎖し、許可を返さない。チャット接続だけでH₂O・Jev・Swarm・記憶管理を稼働中とは表示しない。取込snapshot原文は送らない。

接続・送信・取消はegui外のworkerで実行する。リクエスト既定10秒、応答既定180秒に加え、setup/取消/watchdogの期限を設ける。期限はUIの退役・後着破棄の境界であり、停止したOS/factory threadを強制終了する保証ではない。結果不明の送信は自動再送しない。確定失敗では空の入力欄に下書きを戻す。失敗・切断を固定デモ応答に置換しない。

APIにturn IDがないため、1回の送信ごとに別の接続・隔離sessionを使い、使用後の接続は閉じる。後続送信でも全ツール禁止を再確認する。以前のsession ID再利用や報告経路の変化は送信前に拒否する。画面の準備済み表示は物理socketの常時接続を保証せず、停止したbridgeは次回送信時に検知される場合がある。新しいチャットを作った後の過去履歴は参照専用で、遠隔contextの再開機能ではない。

## 文脈と保存

後続送信では、このチャットで成功応答を得た質問と応答だけをreminderとして添付する。直近から連続する往復を選び、本文のUnicode文字数合計32,000以内、表示順は古→新の時系列。見出し・区切り・省略件数の説明は別に加わる。最新の1往復だけで超過する場合、それより前の文脈もすべて省略する。失敗・取消・結果不明の送信は再送しない。新しい会話では引継ぎをリセットする。

32,000は本文選択の予算であって、今回の入力、全payload、tokens、UI履歴全体の上限ではない。モデル固有context限界は未検証。reminderは非表示のuser-role文字列であり、過去の独立assistant/system-roleを復元したものでも、モデルが強制する信頼境界でもない。対応bridgeが転送しなければ文脈は届かない。

このUI自身の会話履歴はメモリ内のみだが、**実会話の入力・応答・session情報・reminderはdaemon/runtime側に保存される場合がある**。アプリを閉じることや一時作業ディレクトリの削除は、daemon記録の削除証拠ではない。保存期間・削除方針・実際の保持は未確認。注意書きを送信前の画面、履歴、監視表示に設けた。

## 検証と制限

実JcodeClientと制御byte peerによる機能RED→GREEN、取消直後・旧Error末尾・遅延旧応答の独立probe、SDK framing/retraction/final_text、失敗閉鎖、draft/履歴/IME/light-dark/snapshot非転送を検査。PMと最終独立レビューのnative-ui,jevは167成功・5 ignored、exit 0。元の独立probe3/3、再送文脈probe2/2、全文行スクロール1/1、最新往復超過1/1も成功。実サービス/live試験はignoredのまま。

360×480の両テーマで入力・送信・取消を確保し、詳細欄の全行へスクロールで到達できることをヘッドレスで確認した。最小サイズは窮屈で、詳細viewportは約36px。実GUI、DPI、実IME、screen reader、実named pipe、providerの応答品質・文脈の有用性・実効保存方針はUNKNOWN。

以前の不合格・gate失敗・中断レビューを保持し、利用者の継続指示による限定修正後の新規別ベンダーレビューでコード/offline統合passを得た。元目的のSwarm/bot/記憶/Jev/内部言語の実統合や科学的効果は今回実証していない。詳細は`target/2026-11-12-real-chat/state.json`、`evidence/review-continuation.md`、`purpose-alignment.md`、PM試験・binary hash・task_gateログを参照する。
