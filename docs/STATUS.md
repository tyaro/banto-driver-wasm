# STATUS

残作業の再開に必要な現状だけを記す。段階ごとの実装・検証の経緯は git 履歴と各 PR に残っている。

## 現行スナップショット（2026-10-27）

- `origin/main` は #40 までマージ済み。主な機能 PR は #32（Modbus 基本）、#33（register-bit read）、#34 `c9cf3b8`（V 周期収集除外）、#35 `b3d944f`（Q X / R SB・F 診断）、#37（Hub が guest に委ねた SLMP 書込み・診断範囲の境界試験）。#38〜#40 は文書のみ。
- Modbus guest: FC1/FC2 Bool、FC3/FC4 `i16/u16/i32/u32/i64/u64/f32/f64` と 3xxxx/4xxxx `.0`–`.15` Bool を読取る。FC3/FC4 probe は接続 handshake。任意の FC5 単一 coil / FC16 contiguous holding write は `write_mode=enabled` と要求全体を覆う明示 `write_allowlist` を要し、初期値は無効。register-bit write は意図的に送信前 `Unsupported`。
- SLMP guest: 既知 Q/R wire 系列の明示タグ、D/W などの数値 8 型と対応 bit Bool、機種別に確認した読取り経路、既定無効の D/W 数値・M/Y/L/B Bool 書込み試作、任意の diagnostics-v1 を持つ。`Unknown` 範囲は成功読取り後も確定しない。型式・wire が不明なら読取りを送らず、書込みは SD 根拠が不明なら拒否する。V edge relay は周期収集で見逃すため登録から除外する。#35 診断は点数を報告するだけで読書き権限を広げない。
- Hub の保存済み運用ポリシー、権限・監査は guest とは独立した第二ゲート。manifest の `capabilities` は認可ではない。

## 完了

- Hub 側の模擬受入れ: tyaro/banto-industrial#881（#32 固定 guest の Modbus 代表型読取り・FC5/FC16・独立ゲートと監査）、tyaro/banto-industrial#884（#33 固定 guest の register-bit Bool 読取りと送信前 write 拒否）、tyaro/banto-industrial#885（#34 固定 SLMP guest と既存 write policy）、tyaro/banto-industrial#887（#35 診断）。
- Hub の CI が固定するのは #35 の `b3d944f5`（tyaro/banto-industrial#887 で #34 から置き換え。#34 の V の拒否は #35 に含まれる）。
- tyaro/banto-industrial#889 の照合で Hub が guest 試験に依存していた SLMP の書込み範囲根拠と診断の末尾またぎを、#37 で guest の模擬試験として追加した。

## 検証

- Hub 固定 #35 `b3d944f` と main `e4db7a9` の契約差（2026-10-27、ローカル Rust 1.98.1・wasm-tools 1.262.0）: `component`・`drivers`・`wit`・`slmp-component`・比較 crate・ワークスペース設定の差分は `drivers/slmp/README.md` のみ。同一ディレクトリで両版の SLMP guest を debug ビルドし `wasm-tools component new` した成果物は SHA-256 が一致し、別ディレクトリのビルドでも `component wit` の出力は一致した。main の `slmp-runtime` 試験（#37 追加分を含む Wasmtime 31 件・診断 11 件）を #35 の成果物に対して実行し全件成功。よって #37〜#40 は Hub 固定 guest の挙動・WIT を変えず、Hub の固定更新は不要。
- #38 の CI（[run 40534797331](https://github.com/tyaro/banto-driver-wasm/actions/runs/40534797331)）は `changes` / `scaffold` / `slmp-test` / `ci-gate` が成功、他は文書のみの変更のため skipped。ローカルでは `check-scaffold.mjs`、`ci-changes.test.mjs` 17/17、`git diff --check` が成功。
- #37 時点のローカル Rust 1.98.1 で `slmp-runtime` の Wasmtime 試験 31 件と診断試験 11 件、同 crate の fmt・Clippy が成功。

## 未検証

- 実機 PLC への Modbus TCP / SLMP 読書き、#35 診断の実機確認と Hub 表示・品質、実機運用時の監査。
- 模擬試験で網羅していない型・設定・経路。SLMP の全機種・経路別 SD 取得、拡張 D/W の ZR 変換、全タグ種の範囲判定。
- 本番配置・資格情報、GUI、文字列と条件付き範囲の追加適合、配布 manifest の製品受入れ。

## 残件

- 公式表の機種条件、経路、アドレス基数、bit/word 単位、複合タグの全範囲をフィクスチャで検証する。本番前の read-only 接続テストと Hub [Issue #845](https://github.com/tyaro/banto-industrial/issues/845) の境界を合わせ、実機 write は別途承認を受ける。
- tyaro/banto-industrial#881 / tyaro/banto-industrial#884 / tyaro/banto-industrial#885 / tyaro/banto-industrial#887 の模擬受入れを維持し、#35 診断の実機確認、未試験の型・範囲と本番での二重ゲート運用を別作業で照合する。
- 未確定の機種・経路・拡張範囲、指定タグ読取りテストの採否と WIT 互換性は別作業で判断する。tyaro/banto-industrial#871 の gap 読取り最適化は後回し。

## 次作業

1. guest ソースか WIT を変える PR では、Hub の CI が固定する #35 `b3d944f5`（tyaro/banto-industrial#887）との契約差を上記と同じ手順で再確認する。#35 診断の実機確認は別作業で検証する。guest の能力追加だけで本番許可にしない。
2. 実機と Hub の未検証点を個別に確認する。文字列、条件付き機種・経路の範囲、指定タグ読取りテスト、配布 manifest は候補として保持し、採否と実装範囲は別途決める。

必要環境: Rust 1.98.1（Modbus コアの CI は 1.85.1）、`wasm32-unknown-unknown`、wasm-tools 1.262.0、wit-bindgen 0.63.0、テスト依存 Wasmtime 50.0.0、Node.js 24。新しいクラウド環境には前セッションの導入が継承されると仮定せず確認する。実通信の自動確認は制御したローカル TCP で行い、実機 PLC は使わない。

## 判断

将来候補の採否はここでは決めていない。

- 開発場所は Hub と別の公開リポジトリ `tyaro/banto-driver-wasm`。既存ドライバの一括移植はしない。
- SLMP の実設定点数と機種別範囲判定は WASM 側の設計対象とし、Hub は I/O・接続テストの表示・品質・監査を担う。形式/型不正と実機未確認、範囲外と unknown を区別する。
- 既知 Q/R の範囲 unknown のタグは利用者責任で 1 件ずつ明示読取りしてよい。型式不明・0101 拒否時は Q/R wire を確定できないので読まない。
- V 周期収集除外と Modbus register-bit write の `Unsupported` は現行契約として維持する。
- WIT 境界は [CONTRACT.md](CONTRACT.md) に従う。manifest の `settingsVersion` は必須正整数、TCP gap はゼロ。MSRV 宣言と native descriptor の欠落時 `settings_version=1` は変更しない。
- CI は Linux 中心。nextest 分割は計測後に判断する。
