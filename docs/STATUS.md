# STATUS

更新日: 2026-10-24

## 完了

- README、作業ガイド、段階計画、軽量 CI の判定・ガードを用意した。
- 初回コミット `e6cbcf7e32f8299dda1afaf90c7579c01487c6cc` を main に公開した。
- Hub `a0d01aa2` の公開 API・WIT 案・native Modbus 実装を照合し、未決の境界を [CONTRACT.md](CONTRACT.md) に記録した。
- [PR #1](https://github.com/tyaro/banto-driver-wasm/pull/1) を `1e726f0c1bc523e9ef9ea8855699f7b00adb011e` で main にマージした。FC3・単一保持レジスタの依存なし `no_std` コアを追加し、余剰 PDU バイトを拒否する修正まで含む。

## 検証

- Node.js 24.19.0 で `node --test .github/scripts/ci-changes.test.mjs` が 12/12 成功。
- `node .github/scripts/check-scaffold.mjs` と `sdk` / `slmp` 指定のガードが成功。
- `git diff --check` が成功。CI 定義は YAML として解析できた。
- [初回公開の GitHub Actions](https://github.com/tyaro/banto-driver-wasm/actions/runs/40109757859) は全ジョブ成功。
- [STATUS 更新後の GitHub Actions](https://github.com/tyaro/banto-driver-wasm/actions/runs/40109822669) も全ジョブ成功。
- Modbus コアは Rust 1.85.1 で単体テスト 7/7、整形、Clippy、`wasm32-unknown-unknown` への `cargo check` が成功。[PR #1 の最終 CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40111720556) も全ジョブ成功。WASM コンポーネントを実行した結果ではない。
- [PR #1 マージ後の main CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40112148506) は全ジョブ成功。

## 未検証

- SDK、確定 WIT、WASM コンポーネント、Hub loader / runtime、Modbus / SLMP 通信、native との実行結果一致、実機、成果物再利用は未実装・未検証。
- FC3 以外、複数レジスタ、ワード順、文字列、アドレス解析、設定検証も未実装。

## 残件

- `CONTRACT.md` の WIT 境界（通信間隔、probe、設定の版）を Hub 側で決める。独自 ABI を先に作らない。
- 同じフィクスチャで native とコアを実行比較し、公開 API の `FrameDriver` へ接続する。Hub の WASM loader / runtime は別リポジトリで必要。
- SDK / WIT と SLMP に製品ソースを足すときは、CI のガードを実ビルド・テストへ置き換える。WASM 成果物の一度ビルド・再利用と計測は、コンポーネント実装時に行う。

## 次作業

1. 新しいセッションで [CONTRACT.md](CONTRACT.md) の論点を Hub 側の設計と突き合わせ、WIT v1 の確定範囲を判断する。
2. native 比較用の共通フィクスチャを用意し、FC3 の結果一致を自動検証する。二重保守はまだ解消していない。

## 判断

- 開発場所を Hub と別の公開リポジトリ `tyaro/banto-driver-wasm` にする。
- 初回の製品目標は Modbus の WASM 読み取りと native 結果一致。既存ドライバの一括移植はしない。
- CI は Linux 中心。現在の Modbus 領域は Rust の整形・Clippy・単体テスト・WASM ターゲットの型検査を対象とする。WIT コンポーネントのビルドや nextest 分割は後続の計測に基づく。
