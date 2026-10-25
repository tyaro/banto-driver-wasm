# banto-driver-wasm

Banto Hub 向け WASM ドライバ SDK と Modbus TCP・SLMP ドライバの開発場所です。現在は Modbus TCP の FC3・単一タグ U16/U32 読み取りコアと、FC3・単一 U16 のみを扱う実験的な WASM コンポーネントがあります。Hub の loader / runtime は未実装で、Hub にインストールできる配布物はまだありません。

## 境界と初回目標

Hub にある公開 `banto-driver-api`（`FrameDriver`）、`banto-driver-engine`、`banto-driver-conformance`、`DriverRegistry` を再利用する方針です。Hub はソケット、broker の 1 接続 1 セッション、品質判定、書き込みの関門、監査を持ち、WASM はフレームの生成・応答解析・アドレスと設定の検証を担当する想定です。既存 native ドライバは内部依存があるため、クレート一式をこのリポジトリへコピーしません。

最初の製品目標は Modbus TCP の **WASM 読み取り**を Hub の既存 native 実装と同じ入力・応答で比較することです。既存の FC3・単一 U16 はコア、native、実 WASM の共有フィクスチャ12件で比較します。FC3・単一タグ U32（2レジスタ）の `low_high` / `high_low` はコアと native の比較に限定します。U32 の実 WASM 化、文字列、SLMP、書き込み、プラグイン UI、配布管理は後続段階です。初版 WIT 境界は [CONTRACT.md](docs/CONTRACT.md) に記録しています。

## 作業の入口

- [docs/STATUS.md](docs/STATUS.md): 確認済みの進捗、残件、次作業。新しいセッションはここから開始します。
- [docs/PLAN.md](docs/PLAN.md): 段階計画、完了条件、CI の拡張順。
- [docs/CONTRACT.md](docs/CONTRACT.md): 現行 API と承認済み初版 WIT 境界の照合。
- [drivers/slmp/README.md](drivers/slmp/README.md): 標準 Q / iQ-R・ENCPU の SLMP 型式・設定点数・タグ範囲判定の設計案。
- [AGENTS.md](AGENTS.md): 作業・引継ぎの規約。

## 現在の CI

Linux で変更パスの判定テストと文書・ソース追加のガードを実行します。既存の Modbus コア CI は Rust 1.85.1 のまま維持し、WIT コンポーネントと Wasmtime 比較は Rust 1.98.1 を使います。ローカル確認例（`wasm-tools` 1.262.0 と `wasm32-unknown-unknown` が必要）:

```sh
node --test .github/scripts/ci-changes.test.mjs
node .github/scripts/check-scaffold.mjs
cargo fmt --all --check
cargo clippy -p banto-modbus-read-core --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo check -p banto-modbus-read-core --target wasm32-unknown-unknown --locked
cargo +1.98.1 test --manifest-path comparison/Cargo.toml --locked
cargo +1.98.1 test --manifest-path component/Cargo.toml --locked
cargo +1.98.1 build --manifest-path component/Cargo.toml --target wasm32-unknown-unknown --locked
wasm-tools component new component/target/wasm32-unknown-unknown/debug/banto_modbus_tcp_component.wasm -o /tmp/banto-modbus-tcp-component.wasm
wasm-tools validate /tmp/banto-modbus-tcp-component.wasm
BANTO_COMPONENT_WASM=/tmp/banto-modbus-tcp-component.wasm cargo +1.98.1 test --manifest-path runtime-comparison/Cargo.toml --locked
```

比較試験は Hub `a0d01aa2` の公開 `FrameDriver` を固定 Git 依存として取得します。FC3・単一 U16 の応答12件は native、コア、実 WASM コンポーネントで共有し、10件が一致、余剰 PDU の2件は native の受理とコア・WASM の厳密拒否を検査します。U32 の応答15件は native とコアだけを照合し、13件の一致と余剰 PDU 2件の意図的な厳密拒否を検査します。native 接続は `word_order` 未指定時に `low_high` を使用しますが、native の設定フォームの既定表示は `high_low` です。実 WASM は独立した Wasmtime テストホストで実行します。Hub runtime 経由の実行と Modbus / SLMP 通信は未検証です。コアの `wasm32` 型検査と実コンポーネント実行は別の確認です。

## 参照元

- [Hub driver plugin design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-plugin-design.md)
- [Hub driver UI design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-ui-design.md)
- [Hub driver R2b-7 design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-r2b7-design.md)
- [Hub CI](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.github/workflows/ci.yml)、[変更判定](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.github/scripts/ci-changes.mjs)、[nextest 設定](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.config/nextest.toml)
