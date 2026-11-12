# jcode-h2o

[jcode](https://github.com/1jehuang/jcode) をベースにした研究用の試作です。
受付 bot・JCode Swarm・H₂O Memory・Jev による目的逸脱監視を組み合わせ、
人の依頼から成果・履歴までを追跡できるエージェントハーネスを目指しています。

> **状態: 研究用試作（未完成）。** 実 daemon・実 bot・実 Jev との統合や、
> 記憶・監視の実環境での効果はまだ実証していません。未確認の項目は各ドキュメントで
> `unknown` / `UNKNOWN` と明記しています。

## 構成

H2O 関連のコードとドキュメントは、次の 2 か所に分けています。jcode 本体のコードは
upstream のままです。

| パス | 内容 |
| --- | --- |
| [`experiments/h2o-drift/`](experiments/h2o-drift/) | 試作の本体。独立した workspace の crate で、ルートの workspace には入れていません |
| [`docs/vis-harness/`](docs/vis-harness/) | 要求仕様・設計・段階ごとの検証記録 |

`h2o-drift` の主な機能（Cargo の feature ごと）:

- **既定**: H₂O Memory の保存機構（原文の保存と復元、使用中・待機・ディスク退避の 3 相）と、枝ごとの逸脱判定・修正予算・停止
- **`sdk`**: jcode SDK の境界を模擬した監視
- **`jev`**: 実 Jev 判定器 `JevJudge`（故障時は `unknown` に変換）と Swarm 監視用の `h2o-monitor`
- **`ui`**: 127.0.0.1 専用の静的ランチャー `h2o-ui`（スナップショットを読み取り専用で表示）
- **`native-ui`**: Rust / egui のネイティブ受付画面 `h2o-native`（デモモードと、ツールをすべて無効にした SDK セッションによる実チャット接続）

## クイックスタート

リポジトリのルートで、**Git Bash** から実行してください。Windows では
`RUSTC_WRAPPER` を空にする必要があります。

```bash
export RUSTC_WRAPPER=

# 試験（offline、外部通信なし）
cargo test --offline --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-drift --features "sdk jev"

# ネイティブ受付 UI
cargo run --manifest-path experiments/h2o-drift/Cargo.toml --target-dir target/h2o-native --features native-ui --bin h2o-native
```

起動直後は未接続で、勝手に接続や送信はしません。実会話には、既存の jcode daemon /
API bridge と認証済みの provider が必要です。条件と注意点は
[REAL_CHAT.md](docs/vis-harness/REAL_CHAT.md) を参照してください（実会話の内容は
daemon 側に保存される場合があります）。

その他の起動方法やオプションは
[experiments/h2o-drift/README.md](experiments/h2o-drift/README.md) にあります。

## ドキュメント

- [REQUIREMENTS.md](docs/vis-harness/REQUIREMENTS.md): 要求仕様と受入条件
- [DESIGN.md](docs/vis-harness/DESIGN.md): 設計案
- [ACCEPTANCE_MATRIX.md](docs/vis-harness/ACCEPTANCE_MATRIX.md): 要求ごとに、どこまで検証したか
- [CONTROL_BOUNDARIES.md](docs/vis-harness/CONTROL_BOUNDARIES.md): ルーターと監視の設計境界
- 段階ごとの記録: [STAGE02](docs/vis-harness/STAGE02.md) / [STAGE03](docs/vis-harness/STAGE03.md) / [STAGE03_REINFORCEMENT](docs/vis-harness/STAGE03_REINFORCEMENT.md) / [STAGE04](docs/vis-harness/STAGE04.md)
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
