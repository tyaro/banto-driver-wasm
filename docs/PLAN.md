# 段階計画

## 現行段階と完了条件

main `b3d944f` では #32 Modbus 基本、#33 register-bit read、#34 SLMP V 周期収集除外、#35 標準 Q X / 標準 R SB・F 診断がマージ済みです。Modbus は FC1/FC2 Bool、FC3/FC4 数値 8 型と register-bit Bool を読取り、任意の FC5/FC16 write-v1 を既定無効のゲスト設定で制限します。SLMP は既知 Q/R の明示読取り、D/W 数値 8 型と対応 bit Bool、既定無効のゲスト書込み試作、任意の diagnostics-v1 を持ちます。数値 8 型は `i16/u16/i32/u32/i64/u64/f32/f64`。現状と残件は [STATUS](STATUS.md)、個々の経緯は git 履歴と各 PR に残します。

ゲスト実装と段階の製品完了は別です。Hub の CI が固定するのは #35 の `b3d944f5`（tyaro/banto-industrial#887 で #34 `c9cf3b8` から置き換え。#34 の V の拒否は #35 に含まれる）で、#35 診断は tyaro/banto-industrial#887 で模擬受入れ済み。Modbus の保存済み書込みポリシー・運用ゲート・監査は tyaro/banto-industrial#881 の模擬結合試験で確認済みで、tyaro/banto-industrial#884 は register-bit 経路を確認済みです。SLMP #34 は tyaro/banto-industrial#885 の模擬受入れを通過しています。未検証なのは全型・全設定の網羅、実機 I/O、本番配置・資格情報、GUI、#35 診断の実機確認、条件付き範囲の追加証拠、文字列と配布 manifest の製品受入れです。manifest の `capabilities` は認可ではありません。SLMP `Unknown` は確定範囲と混同せず、未確定の機種・wire では安全側に送信を見送ります。V の周期収集除外と Modbus register-bit write の `Unsupported` は意図した現行制限で、修正必須項目ではありません。#35 は診断のみで読書き許可を増やしません。読取り最適化 tyaro/banto-industrial#871 は後回しです。将来候補の採否はこの計画更新では決めません。

| 段階 | 既存の実装・検証 | 製品段階の未完了条件 |
| --- | --- | --- |
| P0–P1 基盤・契約 | README、AGENTS、STATUS、CI 判定・ガード、WIT と Hub API の照合 | 後続の互換性判断は変更時に実施 |
| P2 Modbus | コア・WIT ゲスト・独立 Wasmtime 比較、FC1–FC4 read と gated FC5/FC16 write、register-bit read。tyaro/banto-industrial#881 / tyaro/banto-industrial#884 で固定 guest の代表的な adapter・Hub 経路と独立書込みゲートを模擬検証 | 未試験の型・設定・経路、実機、本番配置、文字列など採用する製品範囲の検証 |
| P3 SLMP | コア・guest の Q/R 読取りと制御フィクスチャ、条件付き点数、diagnostics-v1。tyaro/banto-industrial#885 で #34 固定 guest と既存 write policy を模擬 Hub 受入れ、tyaro/banto-industrial#887 で #35 固定 guest の診断を模擬 Hub 受入れ | #35 診断の実機確認、未試験の機種・経路・型、実機と本番配置 |
| P4 運用拡張 | 任意 write-v1 とバッチ書込みを実装。tyaro/banto-industrial#881 / tyaro/banto-industrial#884 の Modbus と tyaro/banto-industrial#885 の SLMP で既存 Hub ゲート・監査を模擬確認 | 未試験の書込み対象、実 PLC、本番鍵・配置、UI・配布形式の製品確認 |

指定タグの SLMP 読取り試験命令、文字列、機種・経路別範囲拡張、配布管理は将来候補です。0101 型式取得と実際の 0401 読取りは異なる証拠であり、成功値は設定範囲全体の証明になりません。Hub が I/O と接続の世代を管理します。これらを採用する場合は別途契約と検証を定めます。

## CI の拡張順

現在の `changes` は文書、SDK / WIT、Modbus、SLMP、共通・未知パスを分類する。SDK / WIT は両ドライバに波及し、未知パスと共通設定は全領域に倒す。`scaffold` は必須文書と未知ファイルを確認し、`changes` は変更判定テストを実行する。`modbus-test` は Rust 1.85.1 の整形・Clippy・単体テスト・WASM ターゲットの型検査を実行する。`component-build` は WIT と実コンポーネントを検査し、`component-compare` は同じ成果物を Wasmtime で実行して native と照合する。SDK の area ジョブは製品ソースの誤追加を検出し、`slmp-test` は Rust 1.98.1 の整形・Clippy・単体テスト・WASM ターゲット型検査を行う。`ci-gate` は必須ジョブの failure / cancelled / 想定外の skipped を検出する。

SDK に製品コードを導入する際は、その領域の area ガードを実ビルド・テストに置き換える。SLMP の読取りフレームコアには実テストを追加した。現在の Modbus WASM バイナリは `component-build` で一度作り、比較ジョブに artifact として渡す。Rust キャッシュは main で保存し、PR で復元する。PR の古い実行のみキャンセルする。ジョブ・テストの所要時間を記録し、分割数とキャッシュ設定は実測で決める。nextest の分割はまだ行わない。

ソースや依存を増やす変更で追加の承認が必要なら、その時点で止めて状況を報告する。
