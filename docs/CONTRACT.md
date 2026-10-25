# API と WIT の照合

照合元: banto-industrial `aab51f437a303fd3ca4ef77ec73c46e54d43bcbc`（2026-10-25 main）の [API 実装](https://github.com/tyaro/banto-industrial/tree/aab51f43/crates/banto-driver-api)、[Modbus 実装](https://github.com/tyaro/banto-industrial/tree/aab51f43/crates/banto-driver-modbus)、[API 設計 §9.2](https://github.com/tyaro/banto-industrial/blob/aab51f43/docs/banto-driver-api-design.md)、[WIT 案 §6.2](https://github.com/tyaro/banto-industrial/blob/aab51f43/docs/banto-hub-driver-plugin-design.md)。前回の `89de2953` から、照合対象の API、Modbus、engine、WIT 設計に差分はない。本書の次節は承認済みの初版境界であり、Hub の loader / runtime の実装完了を意味しない。

| Hub の現行 API | WIT 案 | 今回の状態 |
| --- | --- | --- |
| `FrameDriver::create_connection(Settings)`、`FrameConnection::handshake_next` | `connection.create`、`handshake-next` | 未実装。Modbus TCP には native と同様にハンドシェイクなしを予定 |
| `FrameConnection::plan_read`、`Step`、`FrameRule::LengthPrefixed` | `read.plan`、`session.step`、`length-prefixed` | FC3・1 レジスタの MBAP 要求バイトだけを純粋関数で実装。transaction ID は呼び手が渡す |
| `FrameConnection::decode_read`、`RawValue`、`ItemError` / `FatalError` | `read.decode`、`types.value`、`item-error` / `fatal-error` | FC3・1 レジスタの `u16` と例外コードを独立型で返す。Hub 型への変換は未実装 |
| `DriverBase::parse_address` / `validate_config` / `settings_schema` | `address.parse` / `config.validate` / `config.schema` | 未実装。現状は 0 始まりのワイヤ offset を渡す |
| `banto-driver-engine`、`DriverRegistry`、適合テスト | Hub の WASM ホストが利用する構想 | 接続なし。Hub に loader / runtime はまだない |

`drivers/modbus` の要求は Hub native の FC3 単一レジスタ要求と同じ MBAP 形である。[独立した比較試験](../comparison/src/lib.rs) は Hub `a0d01aa2` の公開 `FrameDriver` / `FrameConnection` を Git 依存で実行し、同一応答フィクスチャ 12 件をコアへも渡す。正常値、機器例外、ID 不一致、破損フレームの 10 件は値とエラー分類が一致する。残り 2 件は**意図的な差**: native の PDU 解析は宣言したデータの後ろの余剰バイトを受理するが、新コアは正常応答を 4 byte、例外応答を 2 byte に厳格化して `Framing` で拒否する。比較はこの差を明示的に検査する。Hub の engine / WASM 実行は通していない。ワード順、文字列、複数タグ、FC1/2/4 も対象外。

## 初版境界の決定

2026-10-25 に以下の初版方針を承認済み。WIT の `world driver` は設計 §6.2 の `session` / `read` / `address` / `config` を全て export する。読み取り専用 world を使用し、縮小した独自 world や WASI import を作らない。最初のゲストは Modbus TCP の FC3・単一 U16 だけを扱う。書き込み、シリアル、SerialOverTcp、複数レジスタ、文字列は対象外。

1. **接続テスト**: `address` に `use types.{tag-spec}; probe: func() -> option<tag-spec>` を加える。Modbus は `40001` / `u16` を返す。Hub が `parse` してから `read.plan` → 送受信 → `read.decode` を実行し、エラーを成功扱いにしない。`none` の場合だけ handshake 完了で成功とする。Hub の既存テスト経路に合わせ、`connectionId` なし、または既存セッションなしなら直接接続する。保存済み接続が稼働中で設定 fingerprint が一致すればセッションを再利用する。設定変更済みなら別接続せず「保存してからテスト」を返す。`probe` の取得は状態を変えない検証用 export とする。
2. **設定版**: 新しい WASM manifest の `settingsVersion` は必須の正整数で、初版は `1`。欠落、`0`、整数以外は拒否する。`version`（プラグイン SemVer）と `hubApi`（WIT メジャー）とは独立させる。native の古い descriptor に対する欠落時 `1` の補完は変更しない。
3. **通信間隔**: v1 の `transports` は `tcp` のみ。`inter_frame_gap` はゼロとして Hub 側の既存 engine 契約に写す。シリアルと SerialOverTcp をこの契約で受理しない。時間単位と上限を伴う WIT 拡張はシリアル導入時に設計する。
4. **生値と分類**: FC3 の `u16` は WIT `types.value.uint(u64)` に拡張して返す。WASM 境界で U16 の型と `0..=65535` を検査し、範囲外を丸めない。`banto-driver-engine::decode_value` は変更しない。機器例外は `item-error.device`、破損フレームは `fatal-error.framing`、要求と異なる ID は `fatal-error.protocol`。`covers` の範囲・重複と結果件数は既存 `FrameSession` の検査を再利用し、独自の「全タグ必須」条件は足さない。
5. **ホスト起因の失敗**: trap、epoch 超過、メモリ上限はゲストの `fatal-error` ではない。Hub runtime の内部型で原因を保持して構造化診断に記録し、既存 API への正規化を一か所に集約する。接続用の runtime 実行失敗は `Protocol`、受信上限は現行どおり `Framing`、実通信 timeout は現行どおり `Timeout`。`FrameConnection` が `Io` / `Timeout` を返してはいけない現行規則を保つ。失敗した接続用インスタンスは破棄する。検証用の trap・実行上限は検証エラーとし、接続へ波及させない。`memory.grow` の失敗が必ず trap になるとは仮定せず、選択した runtime の実行試験で分類を確かめる。

この決定は製品としての Hub loader / runtime が実装済みであることを意味しない。Hub 本体で必要な最小変更範囲は、manifest の検証、WASM loader / runtime、`FrameConnection` へのアダプター、既存の接続テストへの driver 登録・呼び出しである。Hub の既存担当作業と調整した上で行う。

## 最小比較経路と検証の区別

1. 上記の `world driver` 全 export を持つ読み取り専用ゲストをビルドする。アドレス解析と設定検証は export として実装し、対応しないタグ型や設定は明示的に拒否する。ソケットはゲストに渡さない。
2. 既存の [native 比較試験](../comparison/src/lib.rs) の要求・応答フィクスチャ12件を共通入力として再利用する。実コンポーネントの `read.plan` / `read.decode` を呼び、10件の値・分類一致と余剰 PDU 2件の意図的な厳密拒否を検査する。native の要求 MBAP と `RawValue::Uint` も比較する。
3. `wasm32-unknown-unknown` の `cargo check` はコアの型検査であり、コンポーネントの生成や実行ではない。WIT 型検査、コンポーネント生成、テストホストでの実行、Hub runtime 経由の実行を別々に記録する。Hub runtime が実装されるまで、テストホストの結果を Hub 経由の適合と呼ばない。

照合元は Hub `aab51f43` の `crates/banto-driver-api/src/{driver,frame,error,types}.rs`、`crates/banto-driver-engine/src/{lib,decode}.rs`、`crates/banto-driver-modbus/src/{base,frame_conn}.rs`、`apps/banto-hub/core/src/rest/plc_connections.rs`、`docs/banto-hub-driver-plugin-design.md` §4.2・§6.2–6.4。
