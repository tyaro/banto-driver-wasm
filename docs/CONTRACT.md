# API と WIT の照合

native 比較の固定元: banto-industrial `a0d01aa2` の [API 実装](https://github.com/tyaro/banto-industrial/tree/a0d01aa2/crates/banto-driver-api) と [Modbus 実装](https://github.com/tyaro/banto-industrial/tree/a0d01aa2/crates/banto-driver-modbus)。Hub の実装状態は [PR #843](https://github.com/tyaro/banto-industrial/pull/843) の merge `4c6b126a` に照合する。Hub は既存 WIT の loader / Wasmtime runtime / `FrameConnection` アダプターを持ち、固定ゲスト SHA で FC3・単一 U16 を登録している。このリポジトリの U32 版は Hub に登録されていない。

| Hub の現行 API | WIT 案 | 今回の状態 |
| --- | --- | --- |
| `FrameDriver::create_connection(Settings)`、`FrameConnection::handshake_next` | `connection.create`、`handshake-next` | ゲスト export 実装済み。ハンドシェイクなし。Hub は固定 U16 ゲストへのアダプターを実装済み |
| `FrameConnection::plan_read`、`Step`、`FrameRule::LengthPrefixed` | `read.plan`、`session.step`、`length-prefixed` | FC3・1/2 レジスタの MBAP 要求を実コンポーネントから生成し、transaction ID を進める。Hub の U32 実行は未対応 |
| `FrameConnection::decode_read`、`RawValue`、`ItemError` / `FatalError` | `read.decode`、`types.value`、`item-error` / `fatal-error` | ゲストは U16/U32 を `uint(u64)` で返す。Hub の既存境界は U16 の型・範囲を検査し、U32 は未対応 |
| `DriverBase::parse_address` / `validate_config` / `settings_schema` | `address.parse` / `config.validate` / `config.schema` | ゲストは FC3・U16/U32 アドレス、TCP 設定、word order を扱う。Hub 接続テストは固定 U16 probe を使用 |
| `banto-driver-engine`、`DriverRegistry`、適合テスト | Hub の WASM ホスト | Hub は固定ゲスト SHA の U16 read を登録済み。この U32 版の登録・適合は未実施 |

`drivers/modbus` の要求は Hub native の FC3・単一 U16（1レジスタ）または U32（2レジスタ）要求と同じ MBAP 形である。[独立した比較試験](../comparison/src/lib.rs) は Hub `a0d01aa2` の公開 `FrameDriver` / `FrameConnection` を固定 Git 依存で実行する。U16 の共有フィクスチャ12件と U32 の15件を [Wasmtime 比較試験](../runtime-comparison/src/lib.rs) でも再利用する。U16 は10件、U32 は13件で native・コア・WASM の値と分類が一致する。各2件は**意図的な差**: native の PDU 解析は宣言データ後の余剰バイトを受理するが、コアと WASM は `Framing` で拒否する。この U32 版の Hub runtime 経由実行、文字列、複数タグ、FC1/2/4 は対象外。

U32 の各レジスタ内のバイト順は big endian。固定版 native の `decode_register_value` では `high_low` が先頭を上位16 bit、`low_high` が先頭を下位16 bit として扱う。native の `create_connection` は `word_order` 未指定時に `low_high` を使う一方、native の `settings_schema` のフォーム既定値は `high_low` である。コアとゲストの未指定時は実際の native 接続に合わせて `LowHigh`、ゲストの設定フォーム既定表示も `low_high` とする。`address.parse` は U32 開始 offset 65534（参照番号 `465535`）まで受理し、65535（`465536`）を拒否する。コアは要求生成時にも同じ末尾範囲を検査する。Hub 固定ゲストと Hub 保存値の既定値は変更しない。

## 初版境界の決定

2026-10-25 に以下の初版方針を承認済み。WIT の `world driver` は設計 §6.2 の `session` / `read` / `address` / `config` を全て export する。読み取り専用 world を使用し、縮小した独自 world や WASI import を作らない。Hub 固定ゲストは Modbus TCP の FC3・単一 U16、このリポジトリのゲストは単一 U16/U32 を扱う。書き込み、シリアル、SerialOverTcp、文字列は対象外。

1. **接続テスト**: `address` に `use types.{tag-spec}; probe: func() -> option<tag-spec>` を加える。Modbus は `40001` / `u16` を返す。Hub が `parse` してから `read.plan` → 送受信 → `read.decode` を実行し、エラーを成功扱いにしない。`none` の場合だけ handshake 完了で成功とする。Hub の既存テスト経路に合わせ、`connectionId` なし、または既存セッションなしなら直接接続する。保存済み接続が稼働中で設定 fingerprint が一致すればセッションを再利用する。設定変更済みなら別接続せず「保存してからテスト」を返す。`probe` の取得は状態を変えない検証用 export とする。
2. **設定版**: WASM manifest の `settingsVersion` は必須の正整数で、初版は `1`。欠落、`0`、整数以外は拒否する。U32 追加では表示名とプラグイン `version` を `0.2.0` に更新する。既存の任意 `word_order` 設定を使い、設定移行も WIT 変更もないため `settingsVersion=1` と `hubApi=1` を維持する。native の古い descriptor に対する欠落時 `1` の補完は変更しない。
3. **通信間隔**: v1 の `transports` は `tcp` のみ。`inter_frame_gap` はゼロとして Hub 側の既存 engine 契約に写す。シリアルと SerialOverTcp をこの契約で受理しない。時間単位と上限を伴う WIT 拡張はシリアル導入時に設計する。
4. **生値と分類**: FC3 の `u16` と `u32` は既存 WIT `types.value.uint(u64)` に拡張して返す。WASM 境界で U16 の `0..=65535`、U32 の `0..=4294967295` と型を検査し、範囲外を丸めない。`banto-driver-engine::decode_value` は変更しない。機器例外は `item-error.device`、破損フレームは `fatal-error.framing`、要求と異なる ID は `fatal-error.protocol`。`covers` の範囲・重複と結果件数は既存 `FrameSession` の検査を再利用し、独自の「全タグ必須」条件は足さない。Hub 側では U16 のみ実装済みで、U32 の境界検査は未実装である。
5. **ホスト起因の失敗**: trap、epoch 超過、メモリ上限はゲストの `fatal-error` ではない。Hub runtime の内部型で原因を保持して構造化診断に記録し、既存 API への正規化を一か所に集約する。接続用の runtime 実行失敗は `Protocol`、受信上限は現行どおり `Framing`、実通信 timeout は現行どおり `Timeout`。`FrameConnection` が `Io` / `Timeout` を返してはいけない現行規則を保つ。失敗した接続用インスタンスは破棄する。検証用の trap・実行上限は検証エラーとし、接続へ波及させない。`memory.grow` の失敗が必ず trap になるとは仮定せず、選択した runtime の実行試験で分類を確かめる。

上記の初版方針に沿う Hub loader / runtime と U16 接続テストは PR #843 で実装された。U32 を Hub で利用するには、固定ゲスト SHA の更新、境界での U32 型・範囲検査、接続テスト以外の読取り経路を別作業で照合する必要がある。

独立テストホストは `settingsVersion` の欠落・`0`・負数・小数・文字列・真偽値・`u32` 超過を拒否し、`uint(u64)` の U16/U32 範囲と型を確認する。ローカル TCP の制御した応答で U16 `probe` から `read.decode` まで通す。U32 の15件は実 Wasmtime コンポーネントで照合する。これらは Hub の U32 登録・読取り適合を代替しない。

## 最小比較経路と検証の区別

1. 上記の `world driver` 全 export を持つ読み取り専用ゲストをビルドする。アドレス解析と設定検証は export として実装し、対応しないタグ型や設定は明示的に拒否する。ソケットはゲストに渡さない。
2. 既存の [native 比較試験](../comparison/src/lib.rs) の U16 12件と U32 15件を共通入力として再利用する。実コンポーネントの `read.plan` / `read.decode` を呼び、U16 10件と U32 13件の値・分類一致、各2件の余剰 PDU の意図的な厳密拒否を検査する。要求 MBAP と `uint(u64)` の範囲も比較する。
3. `wasm32-unknown-unknown` の `cargo check` はコアの型検査であり、コンポーネントの生成や実行ではない。WIT 型検査、コンポーネント生成、独立テストホストでの実行、Hub runtime 経由の実行を別々に記録する。この版の Hub U32 適合は未検証である。

照合元は Hub `aab51f43` の `crates/banto-driver-api/src/{driver,frame,error,types}.rs`、`crates/banto-driver-engine/src/{lib,decode}.rs`、`crates/banto-driver-modbus/src/{base,frame_conn}.rs`、`apps/banto-hub/core/src/rest/plc_connections.rs`、`docs/banto-hub-driver-plugin-design.md` §4.2・§6.2–6.4。
