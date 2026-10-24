# 段階計画

状態: 計画。各段階の完了条件を満たすまでは製品機能を実装済みと扱わない。

| 段階 | 作業 | 完了条件 |
| --- | --- | --- |
| P0 初期構成 | README、AGENTS、STATUS、変更判定とガードの CI | ローカルで判定テストと構成ガードが通る。GitHub Actions は公開後に確認する |
| P1 契約確認 | Hub の `FrameDriver` / registry / engine / conformance と WIT 案を照合し、SDK の最小境界を設計 | 公開 API の版・型・所有する I/O・エラー分類が文書とテストで明確。WIT v1 を Hub と同時に更新する手順がある |
| P2 Modbus 読み取り | 最小 WASM SDK と Modbus TCP 読み取りを作る。Hub 側 loader / runtime の受け口と接続する | 同じ要求・応答で native と WASM の値、ワード順、文字列、項目エラー、接続致命エラーが一致。シミュレータで自動検証し、必要な実機確認を記録 |
| P3 SLMP 読み取り | P2 の境界を再利用して SLMP を実装 | native との結果・エラー一致とアドレス表の適合を自動検証 |
| P4 拡張 | 書き込み、UI、配布・更新を Hub の各設計と調整して段階的に追加 | Hub の書き込み関門と監査を維持し、各能力の適合・運用テストを通す |

## CI の拡張順

現在の `changes` は文書、SDK / WIT、Modbus、SLMP、共通・未知パスを分類する。SDK / WIT は両ドライバに波及し、未知パスと共通設定は全領域に倒す。`scaffold` は必須文書と未知ファイルを確認し、`changes` は変更判定テストを実行する。`modbus-test` は Rust の整形・Clippy・単体テスト・WASM ターゲットの型検査を実行する。SDK / WIT と SLMP の area ジョブは製品ソースの誤追加を検出する。`ci-gate` は必須ジョブの failure / cancelled / 想定外の skipped を検出する。

SDK / WIT と SLMP の製品コードを導入する際は、その領域の area ガードを実ビルド・テストに置き換え、依存先にも同じ成果物を渡す。WASM バイナリは 1 ジョブで一度作り、適合・native 比較ジョブに artifact として渡す。Rust キャッシュは main で保存し、PR で復元する。PR の古い実行のみキャンセルする。ジョブ・テストの所要時間を記録し、分割数とキャッシュ設定は実測で決める。現段階で nextest や WASM 成果物のジョブは置かない。

ソースや依存を増やす変更で追加の承認が必要なら、その時点で止めて状況を報告する。
