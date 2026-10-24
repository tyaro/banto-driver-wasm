# STATUS

更新日: 2026-10-24

## 完了

- README、作業ガイド、段階計画、軽量 CI の判定・ガードを用意した。
- 初回コミット `e6cbcf7e32f8299dda1afaf90c7579c01487c6cc` を main に公開した。
- Hub `a0d01aa2` の公開 API・WIT 案・native Modbus 実装を照合し、未決の境界を [CONTRACT.md](CONTRACT.md) に記録した。
- [PR #1](https://github.com/tyaro/banto-driver-wasm/pull/1) を `1e726f0c1bc523e9ef9ea8855699f7b00adb011e` で main にマージした。FC3・単一保持レジスタの依存なし `no_std` コアを追加し、余剰 PDU バイトを拒否する修正まで含む。
- このセッションの開始時に main が `97f5f86c80ca8d5391bab793c98d13b965e67faa` であると確認し、新規ブランチ `fc3-native-compare` を作った。Hub の main は照合元 `a0d01aa2beb007fc2636288c4e839410dece3fdc` と一致した。
- Hub の公開 `FrameDriver` を固定 Git 依存として実行する独立比較クレートを追加した。Hub の製品ソースはコピー・変更していない。WIT の未決事項と推薦案を [CONTRACT.md](CONTRACT.md) に更新した。
- [PR #2](https://github.com/tyaro/banto-driver-wasm/pull/2) を `aed41e6eb8db098da260ee874ca5afebc84d32c8` で main にマージした。比較試験・CI ゲート・契約照合文書を含む。比較元の head は `b758ae90dc3c4152df73b0dcb142f78df913015a`。

## 検証

- Node.js 24.19.0 で `node --test .github/scripts/ci-changes.test.mjs` が 12/12 成功。
- `node .github/scripts/check-scaffold.mjs` と `sdk` / `slmp` 指定のガードが成功。
- `git diff --check` が成功。CI 定義は YAML として解析できた。
- [初回公開の GitHub Actions](https://github.com/tyaro/banto-driver-wasm/actions/runs/40109757859) は全ジョブ成功。
- [STATUS 更新後の GitHub Actions](https://github.com/tyaro/banto-driver-wasm/actions/runs/40109822669) も全ジョブ成功。
- Modbus コアは Rust 1.85.1 で単体テスト 7/7、整形、Clippy、`wasm32-unknown-unknown` への `cargo check` が成功。[PR #1 の最終 CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40111720556) も全ジョブ成功。WASM コンポーネントを実行した結果ではない。
- [PR #1 マージ後の main CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40112148506) は全ジョブ成功。
- `cargo +1.98.1 test --manifest-path comparison/Cargo.toml --locked` で 2/2 成功。FC3 単一レジスタの共通応答フィクスチャ 12 件中 10 件で値・エラー分類が一致。余剰 PDU の正常・例外 2 件は native が受理し、新コアが `Framing` で拒否する意図的な差として検証。要求 MBAP、native の `probe`、`settings_version=1`、TCP の通信間隔ゼロも検証した。
- 今回のローカル確認: Node.js 24.19.0 の変更判定テスト 13/13、構成ガード、Rust 1.85.1 の core テスト 7/7・Clippy・`wasm32-unknown-unknown` の型検査、Rust 1.98.1 の比較テスト 2/2・Clippy・整形、`git diff --check`、CI YAML 解析が成功。[PR #2 の CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40113142990) は `native-compare` / `modbus-test` / `ci-gate` を含む全 7 ジョブ成功。
- [PR #2 マージ後の main CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40113609047) も全 7 ジョブ成功。

## 未検証

- SDK、確定 WIT、WASM コンポーネント、Hub loader / runtime、Modbus / SLMP 通信、実機、成果物再利用は未実装・未検証。native 比較は純粋な FC3・単一レジスタに限定し、WASM と Hub runtime の比較ではない。
- FC3 以外、複数レジスタ、ワード順、文字列、アドレス解析、設定検証も未実装。

## 残件

- `CONTRACT.md` の WIT 境界（通信間隔、probe、設定の版）とホスト起因の trap / 上限エラーの内部分類を Hub 側で決める。独自 ABI を先に作らない。
- 現在の比較範囲を超える型・エラーのフィクスチャを増やし、WIT と Hub loader / runtime の実装後に実 WASM コンポーネントと native を照合する。
- SDK / WIT と SLMP に製品ソースを足すときは、CI のガードを実ビルド・テストへ置き換える。WASM 成果物の一度ビルド・再利用と計測は、コンポーネント実装時に行う。

## 次作業

1. Hub の最新 main の API / 設計を再照合し、`probe`、`settingsVersion`、TCP 限定のゼロ通信間隔、runtime エラー分類の推薦案を Hub 側で判断して WIT v1 の変更範囲を決める。
2. 決定した WIT と Hub loader / runtime に合わせて最小アダプターを実装し、比較を実 WASM へ広げる。ワード順、文字列、複数レジスタ等はまだ範囲外。

## 判断

- 開発場所を Hub と別の公開リポジトリ `tyaro/banto-driver-wasm` にする。
- 初回の製品目標は Modbus の WASM 読み取りと native 結果一致。既存ドライバの一括移植はしない。
- CI は Linux 中心。現在の Modbus 領域は Rust の整形・Clippy・単体テスト・WASM ターゲットの型検査と独立した native 比較を対象とする。WIT コンポーネントのビルドや nextest 分割は後続の計測に基づく。
- native 比較を Modbus 領域の独立 CI ジョブとし、Hub `a0d01aa2` に固定する。余剰 PDU の受理差は新コアの厳格化を維持して明記する。WIT と Hub runtime の契約はこのリポジトリだけで確定しない。
