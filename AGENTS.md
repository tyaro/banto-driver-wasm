# 作業ガイド

最初に [docs/STATUS.md](docs/STATUS.md)、[docs/PLAN.md](docs/PLAN.md)、[docs/CONTRACT.md](docs/CONTRACT.md) を読み、`git status` と既存ファイルを確認する。既存の作業や未追跡ファイルを上書きしない。`.agents/skills` が追加された場合は該当する指示も読む。現時点ではこのリポジトリに専用 skill はない。

## 境界

- Hub の公開 API / `FrameDriver` / registry / engine / conformance を基準にする。WIT や WASM runtime が完成済みだと仮定しない。
- native ドライバは内部依存を持つ。丸ごとコピーせず、初回は Modbus TCP 読み取りの WASM と native の結果一致を狙う。
- ソケット I/O、再接続、品質、書き込みの安全判断と監査は Hub 側。WASM 側の能力は段階ごとに設計・検証する。
- 未実装の製品機能を README、CI、STATUS で「テスト済み」と表現しない。
- `drivers/modbus` は単一 FC3 読み取りの `no_std` コアである。`component` はこのコアを `world driver` に接続する実験的な WIT コンポーネントであり、Hub loader / runtime の実装ではない。

## CI と変更

- `.github/scripts/ci-changes.mjs` の判定を変えたら表テストも変える。未知パスと共通設定は全領域に倒す。SDK / WIT の変更は全利用先へ波及させる。
- Modbus コアは Rust 1.85.1 の整形・Clippy・単体テスト・WASM ターゲットの型検査を CI で維持する。コンポーネントは Rust 1.98.1 で WIT 検査・実ビルド・Wasmtime の比較試験を行う。SDK と SLMP は構成ガードのみ。製品ソースを追加するときは対応する実ビルド・テストを同時に配線し、CI のゲートに必要ジョブを加える。
- 規模が小さい間は nextest の 3 分割などを写さない。WASM 成果物ができたら一度ビルドして後続ジョブで再利用する。main のキャッシュ保存、PR の復元、PR の古い実行だけのキャンセルを維持する。

## 引継ぎ

`docs/STATUS.md` を公開可能な正本として、各段階の完了・検証・未検証・残件・次作業・判断を簡潔に記録する。検証コマンドと結果は区別する。マージと後片付けが終わった時点でも更新して、次のセッションが残件から再開できるようにする。端末固有パス、秘密情報、内部会話は記載しない。
