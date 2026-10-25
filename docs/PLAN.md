# 段階計画

P3 guest follow-up: the separate SLMP component uses the existing WIT world for controlled 0101 -> model-specific basic D SD -> one D/u16 0401 read at a time in Wasmtime. A recognized Q/R wire series permits an explicit unknown-range read at the operator's responsibility; a successful value never proves the range. Unknown model/name-code and 0101 refusal still lack a confirmed Q/R 0401 format in this guest. A structured Hub discovery snapshot is design only. Further WIT/Hub implementation, full device coverage, physical PLC I/O, and writes are later decisions.

P3 follow-up: SD286-289 QnU complete M/B and SD282-285 classified QnUDV internal-only D/W counts are a separate I/O-free stage. Version, serial, and route evidence must be independently confirmed. M/B pairs are complete configured counts, while fast D/W pairs are internal-only; Q SD308-311 totals and tag validation remain later work.

P3 has I/O-free binary 4E 0401 read frames, 0101 model classification, basic SD count handling, conditional QnU ZR SD306-307, and conditional SD282-289 field handling. The current guest uses the existing WIT for its narrow D/u16 read path. Q D/W totals, other tag kinds, structured discovery metadata, Hub integration, PLC I/O, and writes remain later decisions.

状態: 計画。各段階の完了条件を満たすまでは製品機能を実装済みと扱わない。

| 段階 | 作業 | 完了条件 |
| --- | --- | --- |
| P0 初期構成 | README、AGENTS、STATUS、変更判定とガードの CI | ローカルで判定テストと構成ガードが通る。GitHub Actions は公開後に確認する |
| P1 契約確認 | Hub の `FrameDriver` / registry / engine / conformance と WIT 案を照合し、SDK の最小境界を設計 | 公開 API の版・型・所有する I/O・エラー分類が文書とテストで明確。WIT v1 を Hub と同時に更新する手順がある |
| P2 Modbus 読み取り | 最小 WASM SDK と Modbus TCP 読み取りを作る。Hub 側 loader / runtime の受け口と接続する | 同じ要求・応答で native と WASM の値、ワード順、文字列、項目エラー、接続致命エラーが一致。シミュレータで自動検証し、必要な実機確認を記録 |
| P3 SLMP 読み取り | P2 の境界を再利用して SLMP を実装 | native との結果・エラー一致とアドレス表の適合を自動検証 |
| P4 拡張 | 書き込み、UI、配布・更新を Hub の各設計と調整して段階的に追加 | Hub の書き込み関門と監査を維持し、各能力の適合・運用テストを通す |

P2 の段階的追加として FC3・単一タグ U32（2レジスタ）の両 word order を依存なしコア、固定版 native、実 WASM コンポーネントで比較する。U16 の共有12件は維持する。Hub は固定版ゲストで U16 を扱い、U32 の Hub 境界拡張は別工程とする。

P3 では、機種と設定によって異なるデバイスアクセス範囲を確認するため、指定タグの読取りテスト命令を SLMP WASM ドライバに設ける要件を検討する。固定 probe の接続確認とは区別し、静的な形式・範囲検査、実読取りでの機器拒否、通信失敗を分けて扱う。[SLMP Reference Manual](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engm.pdf) の 0101/0000 は機種名の読出し、0401 は連続デバイス読出し、0403 はランダム読出しであり、0101 の成功だけで指定タグのアクセス範囲を保証しない。I/O は Hub エンジンが所有する。具体的な WIT 拡張と機種対応範囲は P3 着手時に提案・確認する。

## CI の拡張順

現在の `changes` は文書、SDK / WIT、Modbus、SLMP、共通・未知パスを分類する。SDK / WIT は両ドライバに波及し、未知パスと共通設定は全領域に倒す。`scaffold` は必須文書と未知ファイルを確認し、`changes` は変更判定テストを実行する。`modbus-test` は Rust 1.85.1 の整形・Clippy・単体テスト・WASM ターゲットの型検査を実行する。`component-build` は WIT と実コンポーネントを検査し、`component-compare` は同じ成果物を Wasmtime で実行して native と照合する。SDK の area ジョブは製品ソースの誤追加を検出し、`slmp-test` は Rust 1.98.1 の整形・Clippy・単体テスト・WASM ターゲット型検査を行う。`ci-gate` は必須ジョブの failure / cancelled / 想定外の skipped を検出する。

SDK に製品コードを導入する際は、その領域の area ガードを実ビルド・テストに置き換える。SLMP の読取りフレームコアには実テストを追加した。現在の Modbus WASM バイナリは `component-build` で一度作り、比較ジョブに artifact として渡す。Rust キャッシュは main で保存し、PR で復元する。PR の古い実行のみキャンセルする。ジョブ・テストの所要時間を記録し、分割数とキャッシュ設定は実測で決める。nextest の分割はまだ行わない。

ソースや依存を増やす変更で追加の承認が必要なら、その時点で止めて状況を報告する。
