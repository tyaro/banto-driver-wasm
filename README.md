# banto-driver-wasm

Banto Hub 向け WASM ドライバ SDK と Modbus TCP・SLMP ドライバの開発場所です。現在は**初期構成のみ**で、SDK、WIT、WASM ドライバ、Hub の loader / runtime は実装していません。利用可能なドライバや配布物はまだありません。

## 境界と初回目標

Hub にある公開 `banto-driver-api`（`FrameDriver`）、`banto-driver-engine`、`banto-driver-conformance`、`DriverRegistry` を再利用する方針です。Hub はソケット、broker の 1 接続 1 セッション、品質判定、書き込みの関門、監査を持ち、WASM はフレームの生成・応答解析・アドレスと設定の検証を担当する想定です。既存 native ドライバは内部依存があるため、クレート一式をこのリポジトリへコピーしません。

最初の製品目標は Modbus TCP の **WASM 読み取り**を Hub の既存 native 実装と同じ入力・応答で比較し、型、ワード順、エラー分類を含めて結果が一致することです。SLMP、書き込み、プラグイン UI、配布管理は後続段階です。WIT の案は [Hub のプラグイン設計](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-plugin-design.md) §6 にありますが、契約として確定していません。

## 作業の入口

- [docs/STATUS.md](docs/STATUS.md): 確認済みの進捗、残件、次作業。新しいセッションはここから開始します。
- [docs/PLAN.md](docs/PLAN.md): 段階計画、完了条件、CI の拡張順。
- [AGENTS.md](AGENTS.md): 作業・引継ぎの規約。

## 現在の CI

Linux 上で Node.js 24 の標準機能だけを使い、変更パスの判定テスト、文書の存在確認、ソース追加時のガードを実行します。ローカル確認:

```sh
node --test .github/scripts/ci-changes.test.mjs
node .github/scripts/check-scaffold.mjs
```

この CI は WASM のビルド、Modbus / SLMP の動作、Hub との適合をまだ検証しません。製品コードを足すときは、対応するビルド・テストジョブを同じ変更で追加してください。

## 参照元

- [Hub driver plugin design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-plugin-design.md)
- [Hub driver UI design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-ui-design.md)
- [Hub driver R2b-7 design](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/docs/banto-hub-driver-r2b7-design.md)
- [Hub CI](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.github/workflows/ci.yml)、[変更判定](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.github/scripts/ci-changes.mjs)、[nextest 設定](https://github.com/tyaro/banto-industrial/blob/a0d01aa2/.config/nextest.toml)
