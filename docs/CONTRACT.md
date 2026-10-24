# API と WIT の照合

照合元: banto-industrial `a0d01aa2` の [API 実装](https://github.com/tyaro/banto-industrial/tree/a0d01aa2/crates/banto-driver-api)、[Modbus 実装](https://github.com/tyaro/banto-industrial/tree/a0d01aa2/crates/banto-driver-modbus)、[API 設計 §9.2](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-driver-api-design.md)、[WIT 案 §6.2](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-plugin-design.md)。本書は照合記録であり、WIT v1 の確定宣言ではない。

| Hub の現行 API | WIT 案 | 今回の状態 |
| --- | --- | --- |
| `FrameDriver::create_connection(Settings)`、`FrameConnection::handshake_next` | `connection.create`、`handshake-next` | 未実装。Modbus TCP には native と同様にハンドシェイクなしを予定 |
| `FrameConnection::plan_read`、`Step`、`FrameRule::LengthPrefixed` | `read.plan`、`session.step`、`length-prefixed` | FC3・1 レジスタの MBAP 要求バイトだけを純粋関数で実装。transaction ID は呼び手が渡す |
| `FrameConnection::decode_read`、`RawValue`、`ItemError` / `FatalError` | `read.decode`、`types.value`、`item-error` / `fatal-error` | FC3・1 レジスタの `u16` と例外コードを独立型で返す。Hub 型への変換は未実装 |
| `DriverBase::parse_address` / `validate_config` / `settings_schema` | `address.parse` / `config.validate` / `config.schema` | 未実装。現状は 0 始まりのワイヤ offset を渡す |
| `banto-driver-engine`、`DriverRegistry`、適合テスト | Hub の WASM ホストが利用する構想 | 接続なし。Hub に loader / runtime はまだない |

`drivers/modbus` の要求は Hub native の FC3 単一レジスタ要求と同じ MBAP 形である。[独立した比較試験](../comparison/src/lib.rs) は Hub `a0d01aa2` の公開 `FrameDriver` / `FrameConnection` を Git 依存で実行し、同一応答フィクスチャ 12 件をコアへも渡す。正常値、機器例外、ID 不一致、破損フレームの 10 件は値とエラー分類が一致する。残り 2 件は**意図的な差**: native の PDU 解析は宣言したデータの後ろの余剰バイトを受理するが、新コアは正常応答を 4 byte、例外応答を 2 byte に厳格化して `Framing` で拒否する。比較はこの差を明示的に検査する。Hub の engine / WASM 実行は通していない。ワード順、文字列、複数タグ、FC1/2/4 も対象外。

## WIT v1 を決める前の論点

1. **通信間隔**: 現行 API の `FrameConnection::inter_frame_gap` は WIT 案にない。engine は送信前にこれを読み、500 ms 超を契約違反として拒否する。比較試験で native Modbus TCP はゼロと確認した。推奨: TCP 限定の v1 ではゼロを契約に明記し、シリアルを許す版で時間の単位と上限を WIT / Hub 両方へ追加する。WIT が間隔を持たないままシリアルを宣言しない。
2. **接続テストと設定版**: 現行 `DriverBase::probe` は接続テスト用の 1 点を返し、native Modbus は `40001` の `U16` を返す。WIT 案の接続テストは handshake 完了だけなので、この点の読み取りを表現できない。推奨: stateless な `probe() -> option<tag-spec>` を address 側に加え、Hub が `parse` → `plan` → `decode` で 1 点読む。`None` は handshake のみ。`DriverDescriptor::settings_version` は native で 1、WIT 案の manifest には `version`（プラグイン版）と `hubApi`（WIT メジャー）しかない。推奨: `settingsVersion: u32` を manifest の必須項目として別に持ち、Hub の設定保存・移行規則と同時に確定する。プラグイン版や Hub API 版から推測しない。
3. **生値**: API `RawValue::{Bool, Int, Uint, Float, Bytes}` は WIT `types.value` と型が対応する。FC3・`U16` の native は `RawValue::Uint(u64)` を返し、engine の `decode_value` は数値タグを `Value::F64` に写す。推奨: 将来のアダプターはコアの `u16` を `uint(u64)` に拡張し、Hub の既存 `decode_value` を再利用する。`float` に先回りして変換しない。型不一致は Hub の `ItemError::Unsupported` にする。文字列の NUL・符号化規則や多語数値は別の比較試験が必要。
4. **trap と上限**: WIT `fatal-error` は `framing` / `protocol` / `invalid-config` だけで、trap・実行時間超過・メモリ上限超過はゲストからの返値ではない。Hub 設計 §6.1、§6.3、§6.4 は接続用のこれらを接続致命としてインスタンスを破棄し、検証用の trap は検証エラーとして接続へ波及させない。`step.max-len` 超と応答タイムアウトもホスト側の接続致命。推奨: Hub runtime にホスト起因の失敗を保持する内部分類を置き、broker へ致命として渡す。文言の解析や `item-error` への変換はしない。`FatalError::InvalidConfig` は接続設定の誤りだけに使う。

照合元は Hub `a0d01aa2` の `crates/banto-driver-api/src/{driver,frame,error,types}.rs`、`crates/banto-driver-engine/src/{lib,decode}.rs`、`crates/banto-driver-modbus/src/{base,frame_conn}.rs`、`docs/banto-hub-driver-plugin-design.md` §4.2・§6.2–6.4。上記は推薦案であり、Hub 側の WIT / runtime を変更・確定していない。

これらを決めるまでは独自 ABI や確定 WIT ファイルを追加しない。FC3 単一レジスタの比較は完了した。次は Hub と WIT v1 の境界を決めてから最小アダプターを設計する。
