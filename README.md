# banto-driver-wasm

## 現在の実装と適用範囲

2026-10-27 の main `c9c3d83`（#44）には PR #32–#35 がマージ済みです。その後、SLMP では同じ接続の SD で範囲外と確定した bit タグを読取り計画から外し（#43）、Modbus では 4xxxx.N register-bit write を FC22 マスク書込みとして追加しました（#44）。Hub の CI の固定は SLMP・Modbus とも #44 `c9c3d83` です。SLMP は tyaro/banto-industrial#895 で #35 `b3d944f5` から更新され、#43 の範囲外除外を含めて模擬 Hub で受入れられました。Modbus は tyaro/banto-industrial#896 で #33 `02a92645` から更新され、FC22 が模擬 Hub で受入れられました。FC22 の実機確認は作者による adapter 層での手動確認のみで、Hub の REST / preflight は経由していません。以下はゲストと I/O を持たないコアの能力です。ゲストの制御フィクスチャと独立 Wasmtime 検証に加え、後述の範囲は模擬 Hub 結合試験を通過しています。上記の FC22 の手動確認を除き、実機 PLC と本番配置は未検証です。

| 対象 | 現在のゲスト能力 | 制限 |
| --- | --- | --- |
| Modbus TCP | FC1/FC2 の Bool、FC3/FC4 の `i16/u16/i32/u32/i64/u64/f32/f64`、3xxxx/4xxxx の `.0`–`.15` register-bit Bool を読取る。FC3/FC4 の設定済み 1 レジスタ probe を接続 handshake で実行する | 3xxxx.N（入力レジスタ）の bit は読取り専用 |
| Modbus TCP 書込み | 任意の `write-v1` world で FC5 単一 coil、FC16 連続 holding words、または FC22 マスク書込みによる 4xxxx.N 単一 bit | 初期値 `write_mode=disabled`。`enabled` と要求全体を含む明示 `write_allowlist` の両方が必要。Hub の保存済み運用ポリシー・権限・監査は独立した第二ゲート。tyaro/banto-industrial#881 の模擬結合試験で FC5/FC16 と拒否経路を確認済み。FC22 の register-bit write は tyaro/banto-industrial#896 で模擬 Hub 受入れ済み。FC22 非対応機器が例外応答を返せば項目単位の `Device` エラー、応答しなければバッチが `Timeout` になり接続を切る。FC22 非対応と分かっている機器は `register_bit_write=disabled`（既定 `fc22`）で、4xxxx.N 書込みを送信前に `Rejected` にできる。実機は作者による adapter 層での手動確認のみ |
| SLMP | 既知 Q/R 系列の明示タグについて、D/W 等の数値 8 型、対応 bit エリアの Bool、機種別に実装した timer/counter・ZR・D word-bit を読取る。任意の diagnostics-v1 は標準 Q X、標準 R SB/F の点数を報告する | `Unknown` は範囲の保証ではない。既知 wire 系列の範囲不明タグは明示的な単独読取りに限り、未知の型式・経路や確定範囲外を推測して読まない。診断点数は読書き許可を広げない |
| SLMP 書込み | ゲスト内の既定無効ポリシーで D/W 数値・M/Y/L/B Bool を検証する制御試作 | 明示 allowlist、同一接続の SD 根拠、Hub の独立運用ゲートが必要。tyaro/banto-industrial#885 は #34 固定の模擬受入れで既存ポリシーを回帰確認。SLMP の Hub の CI の固定は tyaro/banto-industrial#887 で #35 `b3d944f5` に、tyaro/banto-industrial#895 で #44 `c9c3d83` に更新。V edge relay は周期収集タグから意図的に除外する |

数値 8 型は `i16/u16/i32/u32/i64/u64/f32/f64`、bit は Bool です。`low_high` / `high_low` は複数ワード値に適用します。文字列、条件付きの機種・経路別範囲拡張、配布 manifest の製品受入れは未完了で、今後の採否は未決です。読取り最適化 [#871](https://github.com/tyaro/banto-industrial/issues/871) は後回しです。範囲の `Unknown` と構文・wire 未対応の `Unsupported` は区別します。詳細は [Modbus 契約](docs/CONTRACT.md#modbus-wasm-local-contract-update-2026-10-27) と [SLMP 契約](drivers/slmp/README.md) を参照してください。

industrial [#881](https://github.com/tyaro/banto-industrial/pull/881) は #32 固定ゲストで FC2/FC4、数値・Bool（代表型）、FC5/FC16 と独立運用ゲート・監査を模擬 Hub で検証しました。[#884](https://github.com/tyaro/banto-industrial/pull/884) は #33 固定ゲストで register-bit Bool の保存・broker 読取り・REST read-now と送信前 write 拒否を検証しました。[#885](https://github.com/tyaro/banto-industrial/pull/885) は SLMP #34 merge `c9cf3b8` を固定して模擬受入れし、#35 の Q X / R SB・F 診断は [#887](https://github.com/tyaro/banto-industrial/pull/887) で模擬受入れしました。[#895](https://github.com/tyaro/banto-industrial/pull/895) は SLMP の固定を #35 `b3d944f5` から #44 `c9c3d83` に更新し、Q X、R SB/F、書込み handshake の M、SD 拒否・飽和、保存済み M64 の Bad/None・REST 502・未送信を検査する 5 件を模擬 Hub に追加しました（SLMP の期待件数 20→25）。[#896](https://github.com/tyaro/banto-industrial/pull/896) は Modbus の固定を #33 `02a92645` から #44 `c9c3d83` に更新し、FC22 register-bit write の set/clear、二重ゲートの拒否、項目単位の例外、ACK 不一致を模擬 Hub で受入れました。これらは代表的な模擬経路の検証であり、全型・全設定の網羅、実機 PLC、GUI、本番配置を認定しません。Hub の公開 API・ソケット I/O・再接続・品質・書込みの安全判断と監査は Hub 側が所有します。このリポジトリの manifest は能力メタデータであり、配布形式の完成や書込み許可を証明しません。

## 歴史的な初回目標

初回の製品目標は Modbus TCP の FC3・単一 U16 読取りを native と比較することでした。U16 の共有12件、U32 の15件について、独立 Wasmtime 実行まで拡張しました。各2件の余剰 PDU は native が受理しゲストが厳密拒否する意図的な差です。Hub [PR #843](https://github.com/tyaro/banto-industrial/pull/843) の固定 U16 接続試験は、このリポジトリの現在の全能力の Hub 受入れを示すものではありません。

## 作業の入口

- `docs/STATUS.md`: 現行スナップショット、未検証点、残件を記すローカルの引継ぎメモ。git では追跡しない（`.gitignore`）。段階ごとの経緯は git 履歴と各 PR を参照。
- [docs/PLAN.md](docs/PLAN.md): 段階計画、完了条件、CI の拡張順。
- [docs/CONTRACT.md](docs/CONTRACT.md): 現行 API と承認済み初版 WIT 境界の照合。
- [drivers/slmp/README.md](drivers/slmp/README.md): SLMP の実装済み能力、制限、範囲判定と今後の設計候補。
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
