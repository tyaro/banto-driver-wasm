# banto-driver-wasm


The SLMP I/O-free core also plans conditional QnU complete M/B counts (SD286-289) and recognized QnUDV internal-only D/W counts (SD282-285). These are field counts; D/W totals and complete tag ranges are not inferred; see [`drivers/slmp`](drivers/slmp/README.md) for evidence requirements.

SLMP contains an I/O-free binary 4E 0401 read-frame core, 0101 model classification, basic SD point counts, conditional QnU ZR counts, and the conditional Q SD282-289 fields described below. Hub integration, PLC communication, and remaining device-range decisions are later stages.

Banto Hub 向け WASM ドライバ SDK と Modbus TCP・SLMP ドライバの開発場所です。現在は Modbus TCP の FC3・単一タグ U16/U32 読み取りコアと、同じ範囲を扱う WASM コンポーネントがあります。Hub は [PR #843](https://github.com/tyaro/banto-industrial/pull/843) で FC3・単一 U16 の WASM ゲストを固定 SHA で登録しました。このリポジトリの U32 版コンポーネントは Hub に登録されておらず、Hub 経由の U32 は未対応です。

## 境界と初回目標

Hub にある公開 `banto-driver-api`（`FrameDriver`）、`banto-driver-engine`、`banto-driver-conformance`、`DriverRegistry` を再利用する方針です。Hub はソケット、broker の 1 接続 1 セッション、品質判定、書き込みの関門、監査を持ち、WASM はフレームの生成・応答解析・アドレスと設定の検証を担当する想定です。既存 native ドライバは内部依存があるため、クレート一式をこのリポジトリへコピーしません。

最初の製品目標は Modbus TCP の **WASM 読み取り**を Hub の既存 native 実装と同じ入力・応答で比較することです。FC3・単一 U16 はコア、native、実 WASM の共有フィクスチャ12件、単一タグ U32（2レジスタ）の両 `word_order` は15件で比較します。U32 は既存 WIT の `uint(u64)` と `word-count=2` で表現します。文字列、SLMP の実設定範囲・WIT 接続、書き込み、プラグイン UI、配布管理は後続段階です。初版 WIT 境界は [CONTRACT.md](docs/CONTRACT.md) に記録しています。

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
cargo +1.98.1 fmt --all --check
cargo +1.98.1 clippy -p banto-modbus-read-core --all-targets --locked -- -D warnings
cargo +1.98.1 test --workspace --locked
cargo +1.98.1 check -p banto-modbus-read-core --target wasm32-unknown-unknown --locked
cargo +1.98.1 test --manifest-path comparison/Cargo.toml --locked
cargo +1.98.1 test --manifest-path component/Cargo.toml --locked
cargo +1.98.1 build --manifest-path component/Cargo.toml --target wasm32-unknown-unknown --locked
wasm-tools component new component/target/wasm32-unknown-unknown/debug/banto_modbus_tcp_component.wasm -o /tmp/banto-modbus-tcp-component.wasm
wasm-tools validate /tmp/banto-modbus-tcp-component.wasm
BANTO_COMPONENT_WASM=/tmp/banto-modbus-tcp-component.wasm cargo +1.98.1 test --manifest-path runtime-comparison/Cargo.toml --locked
```

比較試験は Hub `a0d01aa2` の公開 `FrameDriver` を固定 Git 依存として取得します。FC3・単一 U16 の応答12件は native、コア、実 WASM コンポーネントで共有し、10件が一致、余剰 PDU の2件は native の受理とコア・WASM の厳密拒否を検査します。U32 の応答15件も同じ3経路で照合し、13件の一致と余剰 PDU 2件の意図的な厳密拒否を検査します。native 接続と本ゲストは `word_order` 未指定時に `low_high` を使用します。native の設定フォーム既定表示は `high_low` ですが、ゲストのフォームは実際の接続既定に合わせて `low_high` とします。実 WASM は独立した Wasmtime テストホストで実行します。Hub の U16 経路は実装済みですが、この U32 版の Hub 経由実行と実機 Modbus / SLMP 通信は未検証です。

## 参照元

- [Hub driver plugin design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-plugin-design.md)
- [Hub driver UI design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-ui-design.md)
- [Hub driver R2b-7 design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-r2b7-design.md)
- [Hub CI](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.github/workflows/ci.yml)、[変更判定](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.github/scripts/ci-changes.mjs)、[nextest 設定](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.config/nextest.toml)
