# banto-driver-wasm

Banto Hub 向け WASM ドライバ SDK と Modbus TCP・SLMP ドライバの開発場所です。現在は Modbus TCP の**読み取りフレーム処理の小さなコア**だけがあります。SDK、WIT コンポーネント、Hub の loader / runtime は実装していません。Hub で利用可能な WASM ドライバや配布物はまだありません。

## 境界と初回目標

Hub にある公開 `banto-driver-api`（`FrameDriver`）、`banto-driver-engine`、`banto-driver-conformance`、`DriverRegistry` を再利用する方針です。Hub はソケット、broker の 1 接続 1 セッション、品質判定、書き込みの関門、監査を持ち、WASM はフレームの生成・応答解析・アドレスと設定の検証を担当する想定です。既存 native ドライバは内部依存があるため、クレート一式をこのリポジトリへコピーしません。

最初の製品目標は Modbus TCP の **WASM 読み取り**を Hub の既存 native 実装と同じ入力・応答で比較し、型、ワード順、エラー分類を含めて結果が一致することです。SLMP、書き込み、プラグイン UI、配布管理は後続段階です。WIT の案は [Hub のプラグイン設計](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-plugin-design.md) §6 にありますが、契約として確定していません。

## 作業の入口

- [docs/STATUS.md](docs/STATUS.md): 確認済みの進捗、残件、次作業。新しいセッションはここから開始します。
- [docs/PLAN.md](docs/PLAN.md): 段階計画、完了条件、CI の拡張順。
- [docs/CONTRACT.md](docs/CONTRACT.md): 現行 API と WIT 案の照合、未決の境界。
- [AGENTS.md](AGENTS.md): 作業・引継ぎの規約。

## 現在の CI

Linux で変更パスの判定テストと文書・ソース追加のガードを実行します。Modbus 領域は Rust 1.85.1 で整形、Clippy、単体テスト、WASM ターゲットへの型検査を実行します。ローカル確認:

```sh
node --test .github/scripts/ci-changes.test.mjs
node .github/scripts/check-scaffold.mjs
cargo fmt --all --check
cargo clippy -p banto-modbus-read-core --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo check -p banto-modbus-read-core --target wasm32-unknown-unknown --locked
cargo +1.98.1 test --manifest-path comparison/Cargo.toml --locked
```

比較試験は Hub `a0d01aa2` の公開 `FrameDriver` を Git 依存として取得し、FC3・単一レジスタの同じ応答 12 件を native と新コアへ渡します。10 件は一致し、余剰 PDU の 2 件は native の受理と新コアの拒否を明示的に検査します。比較ジョブは Hub 側の固定ツールチェーン Rust 1.98.1 を使います。WIT コンポーネントのビルド、Hub runtime での WASM 実行、Modbus / SLMP 通信は未検証です。`wasm32` の型検査は実行可能な WASM ドライバの生成を意味しません。

## 参照元

- [Hub driver plugin design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-plugin-design.md)
- [Hub driver UI design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-ui-design.md)
- [Hub driver R2b-7 design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-r2b7-design.md)
- [Hub CI](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.github/workflows/ci.yml)、[変更判定](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.github/scripts/ci-changes.mjs)、[nextest 設定](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.config/nextest.toml)
