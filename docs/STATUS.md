# STATUS

更新日: 2026-10-25

## 完了

- README、作業ガイド、段階計画、軽量 CI の判定・ガードを用意した。
- 初回コミット `e6cbcf7e32f8299dda1afaf90c7579c01487c6cc` を main に公開した。
- Hub `a0d01aa2` の公開 API・WIT 案・native Modbus 実装を照合し、未決の境界を [CONTRACT.md](CONTRACT.md) に記録した。
- [PR #1](https://github.com/tyaro/banto-driver-wasm/pull/1) を `1e726f0c1bc523e9ef9ea8855699f7b00adb011e` で main にマージした。FC3・単一保持レジスタの依存なし `no_std` コアを追加し、余剰 PDU バイトを拒否する修正まで含む。
- このセッションの開始時に main が `97f5f86c80ca8d5391bab793c98d13b965e67faa` であると確認し、新規ブランチ `fc3-native-compare` を作った。Hub の main は照合元 `a0d01aa2beb007fc2636288c4e839410dece3fdc` と一致した。
- Hub の公開 `FrameDriver` を固定 Git 依存として実行する独立比較クレートを追加した。Hub の製品ソースはコピー・変更していない。WIT の未決事項と推薦案を [CONTRACT.md](CONTRACT.md) に更新した。
- [PR #2](https://github.com/tyaro/banto-driver-wasm/pull/2) を `aed41e6eb8db098da260ee874ca5afebc84d32c8` で main にマージした。比較試験・CI ゲート・契約照合文書を含む。比較元の head は `b758ae90dc3c4152df73b0dcb142f78df913015a`。
- 新規セッションで driver-wasm の main `77b9d8f7c0b8a62e190b3aa7f4f97462714686ae` を clean と確認した。Hub main `89de2953b59587839d93a182c0bc7c17179adb0e` を取得し、前回の `a0d01aa2` から API・Modbus・engine・WIT 設計に差分がないと確認した。未決境界の提案と決定後の比較経路を [CONTRACT.md](CONTRACT.md) に具体化した。製品コードと Hub 本体には変更を加えていない。
- 2026-10-25 に WIT 境界の5案について最終方針の承認を受けた。ローカル準備コミット `02b8fe1c63c1876ee7ba34ba457e0808dde4e5d1` を保持したまま再開し、driver-wasm main `77b9d8f7` と Hub main `aab51f437a303fd3ca4ef77ec73c46e54d43bcbc` を確認した。Hub の対象 API・Modbus・engine・WIT 設計は前回の `89de2953` から差分がない。承認された範囲を [CONTRACT.md](CONTRACT.md) に記録した。
- 承認済み境界に沿う [正本 WIT](../wit/driver.wit)、[FC3・単一 U16 ゲスト](../component/src/lib.rs)、[実 WASM 比較](../runtime-comparison/src/lib.rs) を追加した。`session` / `read` / `address` / `config` の全 export を持ち、ソケットと WASI は持たない。既存の native 比較フィクスチャを共用する。Hub 本体は変更していない。
- CI に `component-build` と `component-compare` を加え、WIT 検査、コンポーネント生成・検証、成果物を使った Wasmtime 実行を配線した。既存の Rust 1.85.1 コア CI と native 比較は維持した。`component/manifest.json` は初版メタデータの試案であり、Hub 側の検証器や配布形式ではない。
- 新規セッションで main `8a6d0a5d03e4e300731930a2c6ed685d28fbbc42`、本書・CONTRACT・両リポジトリの作業規約を確認した。Hub main は `aab51f437a303fd3ca4ef77ec73c46e54d43bcbc`。Hub の並行作業 [PR #842](https://github.com/tyaro/banto-industrial/pull/842) の変更ファイルを照合し、Hub 本体や別担当の作業ツリーは変更していない。独立ブランチ `wasm-host-prep` でテストホストの準備を進めた。
- 独立 Wasmtime ホストに試案 manifest の `settingsVersion` 必須正整数（`u32` に収まる値）・WIT `uint(u64)` の U16 型と範囲の拒否試験を追加した。ローカル TCP の制御した応答で `probe` → `parse` → `read.plan` → 送受信 → `read.decode` を通し、正常値・機器例外・破損・ID 不一致を確認する試験も追加した。これは Hub loader や `FrameSession` 経由の試験ではない。

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
- 今回の文書変更では Node.js 24.19.0 の `node --test .github/scripts/ci-changes.test.mjs` が 13/13 成功し、`node .github/scripts/check-scaffold.mjs` の通常・`sdk`・`slmp` 指定と `git diff --check` が成功した。
- このクラウド環境では公式 rustup から Rust 1.98.1 のみを導入し、`wasm32-unknown-unknown`、wasm-tools 1.262.0、wit-bindgen 0.63.0 を使った。Wasmtime 50.0.0 は比較クレートのライブラリとして使用し、CLI は導入していない。Rust 1.85.1 は新規導入していない。
- 新規 WIT は `wasm-tools component wit` と `validate` に成功。`cargo +1.98.1 build --manifest-path component/Cargo.toml --target wasm32-unknown-unknown --locked` でコアモジュールを作り、`wasm-tools component new`、`validate`、`component targets ... --world driver` に成功。これはコアの `wasm32` 型検査とは別の実コンポーネント生成である。
- `BANTO_COMPONENT_WASM=... cargo +1.98.1 test --manifest-path runtime-comparison/Cargo.toml --locked` は3/3成功。実コンポーネントを Wasmtime 50.0.0 でインスタンス化し、共有12フィクスチャの10一致・2厳密拒否、probe / parse / config / handshake / plan を検査した。別テストで Wasmtime の `memory.grow` 上限失敗が `-1` を返し trap ではないこと、`unreachable` trap と epoch 割込を型付き原因として区別できることを確認した。
- 最終ローカル確認: Rust 1.98.1 でコア単体7/7、コンポーネント単体4/4・doc1/1、native 比較2/2、実 WASM 比較3/3、各クレートの整形・Clippy、コア `wasm32` 型検査が成功。Node.js 24.19.0 の変更判定15/15、構成ガードの通常・`sdk`・`slmp`、CI YAML 解析、manifest の現値確認、`git diff --check` も成功した。
- [PR #3](https://github.com/tyaro/banto-driver-wasm/pull/3) を Ready で公開した。CI の成果物パスをリポジトリ基準の絶対パスに修正し、同じ配置で実 WASM 比較3/3を再実行した。[修正版 CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40150441856) は `component-build` / `component-compare` / `ci-gate` を含む全9ジョブ成功。Rust 1.85.1 のコア CI も成功した。修正前の実行は次の push によりキャンセルされた。
- PR #3 の最終 head `742aa507ef8304258ca4a61bcbf8e737f0214383` は [CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40150933235) 全9ジョブ成功、未解決レビューなしと確認し、`9e1a77010d62276736718a7c6f832c296e5aa6c6` で main に squash merge した。main を同 SHA へ fast-forward して引き継いだ。
- PR #3 後の [main CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40151386077) は `component-build` / `component-compare` / `ci-gate` を含む全9ジョブ成功と確認した。これはこのセッションのテストホスト追加前の main に対する結果である。
- 新規セッションの環境では公式 rustup から Rust 1.98.1 のみを導入し、`wasm32-unknown-unknown`、wasm-tools 1.262.0、wit-bindgen 0.63.0（コンポーネント依存）、Wasmtime 50.0.0（テスト依存）を確認した。Rust 1.85.1 は導入していない。正本 WIT の検証、実コンポーネントのビルド・検証・world 対応、コンポーネント単体4/4・doc1/1、独立 Wasmtime 比較6/6、runtime-comparison の整形・Clippy、Node 変更判定15/15・構成ガード・`git diff --check` が成功。6件には manifest・U16 拒否とローカル TCP 制御通信を含む。

## 未検証

- Hub loader / runtime、manifest の必須正整数検証、`FrameConnection` アダプター、既存接続テストへの登録・呼び出しは未実装。WASM 境界での U16 型・範囲検査、`covers` と結果件数の既存 `FrameSession` 検査への接続、内部原因の構造化診断と API 変換も Hub 側では未検証。
- 実機への Modbus TCP 通信、SLMP、FC3 以外、複数レジスタ、ワード順、文字列、書き込みは未実装または未検証。ローカル TCP の制御通信は独立テストホスト経由のみ。アドレス解析と設定検証はゲスト内だけで実装した。
- ローカルでは Rust 1.85.1 を導入・実行していない。CI では既存の 1.85.1 コア検証と WASM artifact のジョブ間再利用が成功した。実コンポーネントの試験は Wasmtime テストホスト経由であり、Hub runtime 経由ではない。

## 残件

- Hub 側の manifest 検証、loader / runtime、`FrameConnection` アダプター、既存接続テスト経路への登録を既存担当作業と調整する。接続テストの fingerprint による再利用・設定変更拒否と `probe` のエラー伝播は Hub 側で実装する。
- Hub 境界で FC3・U16 の型と範囲を検査し、既存 `FrameSession` の `covers` / 件数チェックと値変換を再利用する。runtime の trap / epoch / memory limit を内部で分類して診断に残し、既存 API への写像を一か所に置く。
- さらに実通信と既存 Hub 接続テスト経由の比較を行う。SDK / WIT と SLMP に製品ソースを足すときは area ガードを実ビルド・テストへ置き換える。

## 次作業

1. Hub 側の既存担当作業と衝突しない変更範囲を確認する。最小範囲は新 WASM manifest の `settingsVersion` 必須正整数検証、loader / Wasmtime runtime、`FrameConnection` アダプター、既存 `FrameSession` と接続テストへの登録である。このリポジトリの `component/manifest.json` は試案であり、Hub の受理形式はまだ実装されていない。
2. Hub 境界で U16 `uint(u64)` の型と `0..=65535` を検査し、既存 `FrameSession` の `covers` / 結果件数検査を再利用する。trap / epoch / memory limit の内部原因と既存 API への写像を実装・検証する。接続テストでは `probe` のエラー伝播、保存設定 fingerprint によるセッション再利用と設定変更拒否を確認する。
3. 実 Modbus TCP 通信と Hub 経由の native / WASM 比較を実施する。FC3・単一 U16・TCP・読み取り専用を維持し、シリアル、複数レジスタ、文字列、書き込みへ広げる判断は別途行う。

変更範囲の案: (1) このリポジトリの `runtime-comparison` で契約・ローカル通信を準備、(2) 調整後に Hub の新規 `crates/banto-driver-wasm`（`banto-driver-api`、Wasmtime 50.0.0、serde 系）で manifest / loader / runtime と `FrameConnection` 変換、(3) `crates/banto-drivers/src/lib.rs` と `apps/banto-hub/host/Cargo.toml` の登録、`apps/banto-hub/core/src/rest/plc_connections.rs` の既存接続テスト経路・局所試験へ接続。Hub [PR #842](https://github.com/tyaro/banto-industrial/pull/842) が変更中の `apps/banto-hub/core/src/{runtime,hub,broker_glue}.rs` と `crates/banto-tags` はこの段階では触れない。Hub 本体への書込みは調整結果を待つ。

## 判断

- 開発場所を Hub と別の公開リポジトリ `tyaro/banto-driver-wasm` にする。
- 初回の製品目標は Modbus の WASM 読み取りと native 結果一致。既存ドライバの一括移植はしない。
- CI は Linux 中心。Modbus 領域は既存の Rust 1.85.1 コア検証、Rust 1.98.1 の native 比較、WIT 検査・実コンポーネントのビルド・Wasmtime 比較を対象とする。nextest 分割は計測後に判断する。
- native 比較を Modbus 領域の独立 CI ジョブとし、Hub `a0d01aa2` に固定する。余剰 PDU の受理差は新コアの厳格化を維持して明記する。WIT と Hub runtime の契約はこのリポジトリだけで確定しない。
- 初版の WIT 境界と適用範囲は [CONTRACT.md](CONTRACT.md) の「初版境界の決定」に従う。新 WASM manifest の `settingsVersion` は必須正整数、TCP gap はゼロ、FC3・単一 U16 の生値は `uint(u64)`、host runtime 失敗は内部で型付き分類とする。
