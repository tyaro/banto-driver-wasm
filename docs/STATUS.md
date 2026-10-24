# STATUS

更新日: 2026-10-24

## 完了

- README、作業ガイド、段階計画、軽量 CI の判定・ガードを用意した。
- 初回コミット `e6cbcf7e32f8299dda1afaf90c7579c01487c6cc` を main に公開した。

## 検証

- Node.js 24.19.0 で `node --test .github/scripts/ci-changes.test.mjs` が 12/12 成功。
- `node .github/scripts/check-scaffold.mjs` と `sdk` / `modbus` / `slmp` 指定のガードが成功。
- `git diff --check` が成功。CI 定義は YAML として解析できた。
- [初回公開の GitHub Actions](https://github.com/tyaro/banto-driver-wasm/actions/runs/40109757859) は全ジョブ成功。

## 未検証

- SDK、WIT、WASM ドライバ、Hub loader / runtime、Modbus / SLMP 通信、native との一致、実機、成果物再利用。いずれも未実装。

## 残件

- 公開 API と native 実装の実際の依存境界を調べ、SDK/WIT v1 の契約を Hub 側と揃える。
- Modbus TCP の WASM 読み取りと一致テストを実装する。Hub 側の受け口は別リポジトリで必要。
- 製品コード導入時に CI の area ガードをビルド・テストへ置き換え、キャッシュ・成果物・計測を実装する。

## 次作業

1. `docs/PLAN.md` の P1 に沿って Hub 側の API と WIT 案を照合する。
2. 製品コードの最初の PR で、実行可能なビルド・適合テストと CI 配線を同時に入れる。

## 判断

- 開発場所を Hub と別の公開リポジトリ `tyaro/banto-driver-wasm` にする。
- 初回の製品目標は Modbus の WASM 読み取りと native 結果一致。既存ドライバの一括移植はしない。
- CI は Linux 中心で開始し、現段階は構成の検査に限る。WASM ビルドや nextest 分割は製品コードができてから計測に基づいて追加する。
