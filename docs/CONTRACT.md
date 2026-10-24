# API と WIT の照合

照合元: banto-industrial `a0d01aa2` の [API 実装](https://github.com/tyaro/banto-industrial/tree/a0d01aa2/crates/banto-driver-api)、[Modbus 実装](https://github.com/tyaro/banto-industrial/tree/a0d01aa2/crates/banto-driver-modbus)、[API 設計 §9.2](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-driver-api-design.md)、[WIT 案 §6.2](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-plugin-design.md)。本書は照合記録であり、WIT v1 の確定宣言ではない。

| Hub の現行 API | WIT 案 | 今回の状態 |
| --- | --- | --- |
| `FrameDriver::create_connection(Settings)`、`FrameConnection::handshake_next` | `connection.create`、`handshake-next` | 未実装。Modbus TCP には native と同様にハンドシェイクなしを予定 |
| `FrameConnection::plan_read`、`Step`、`FrameRule::LengthPrefixed` | `read.plan`、`session.step`、`length-prefixed` | FC3・1 レジスタの MBAP 要求バイトだけを純粋関数で実装。transaction ID は呼び手が渡す |
| `FrameConnection::decode_read`、`RawValue`、`ItemError` / `FatalError` | `read.decode`、`types.value`、`item-error` / `fatal-error` | FC3・1 レジスタの `u16` と例外コードを独立型で返す。Hub 型への変換は未実装 |
| `DriverBase::parse_address` / `validate_config` / `settings_schema` | `address.parse` / `config.validate` / `config.schema` | 未実装。現状は 0 始まりのワイヤ offset を渡す |
| `banto-driver-engine`、`DriverRegistry`、適合テスト | Hub の WASM ホストが利用する構想 | 接続なし。Hub に loader / runtime はまだない |

`drivers/modbus` の要求は Hub native の `build_request_frame(transaction_id, unit_id, 0x03, offset, 1)` と同じ MBAP 形を単体テストで固定した。正常応答・例外・ID 不一致・破損フレームを検査したが、**native の実行結果との自動比較は未実施**。数値のワード順、文字列、複数タグ、FC1/2/4、アドレス正規化も対象外。

## WIT v1 を決める前の論点

1. 現行 API の `FrameConnection::inter_frame_gap` は WIT 案にない。Modbus TCP では既定のゼロで足りるが、後のシリアルドライバには境界を決める必要がある。案: TCP v1 はゼロ固定と明記し、シリアル対応時に WIT と Hub 側を一緒に拡張する。
2. 現行 `DriverBase::probe` と `DriverDescriptor::settings_version` は WIT 案の関数表にない。案: 接続テスト用の probe と設定の版をプラグイン manifest に持つかを Hub の設計と同時に決める。設定の版を黙って 1 と扱わない。
3. 現行 API crate はこの照合元で `0.3.0`、WIT は案のまま。WASM ホスト側の API 版、`RawValue::Uint` への変換、trap / 上限超過の分類を、Hub の実装と適合テストで固定する。API をこのリポジトリへコピーしない。

これらを決めるまでは独自 ABI や確定 WIT ファイルを追加しない。次の小さい単位は、Hub 側で固定した要求・応答フィクスチャを両実装へ同じ入力として渡す比較テストと、公開 API を使うアダプターの設計である。
