# 段階計画

## 現行段階と完了条件

main `25b5a1b`（#48）では #32 Modbus 基本、#33 register-bit read、#34 SLMP V 周期収集除外、#35 標準 Q X / 標準 R SB・F 診断、#43 SLMP 範囲外 bit の読取り除外、#44 Modbus FC22 register-bit write、#47 WIT の `item-error.verification-failed` 追加（ゲスト未使用）がマージ済みです。Modbus は FC1/FC2 Bool、FC3/FC4 数値 8 型と register-bit Bool を読取り、任意の FC5/FC16 write-v1 と 4xxxx.N の FC22 register-bit write を既定無効のゲスト設定で制限します（FC22 は tyaro/banto-industrial#896 で模擬 Hub 受入れ済み）。SLMP は既知 Q/R の明示読取り、D/W 数値 8 型と対応 bit Bool、既定無効のゲスト書込み試作、任意の diagnostics-v1 を持ち、同じ接続の SD で exact 点数を得た bit 領域では範囲外のタグを読みません（#43 は tyaro/banto-industrial#895 で模擬 Hub 受入れ済み）。数値 8 型は `i16/u16/i32/u32/i64/u64/f32/f64`。現状と残件はローカルの引継ぎメモ `docs/STATUS.md`（git 追跡外）、個々の経緯は git 履歴と各 PR に残します。

ゲスト実装と段階の製品完了は別です。Hub の CI が固定するのは SLMP・Modbus とも #47 の `3b7f0f6` です（tyaro/banto-industrial#905 で #44 `c9c3d83` から置き換え。コードは #44 と同じ）。#44 へは、SLMP が tyaro/banto-industrial#895 で #35 `b3d944f5` から（#35 は tyaro/banto-industrial#887 で #34 `c9cf3b8` から）、Modbus が tyaro/banto-industrial#896 で #33 `02a92645` から置き換えました。#47 は WIT の variant に case を足したので、それより前の WIT で作ったゲストは Hub ホストに読み込めません。Hub の古い回帰用の固定は、各コードのまま `wit/driver.wit` だけを #47 のものに差し替えてビルドします。WIT を変える PR は、このように Hub の全固定に波及します。#35 診断は tyaro/banto-industrial#887、#43 の範囲外除外は tyaro/banto-industrial#895、FC22 register-bit write は tyaro/banto-industrial#896 で模擬受入れ済み。Modbus の保存済み書込みポリシー・運用ゲート・監査は tyaro/banto-industrial#881 の模擬結合試験で確認済みで、tyaro/banto-industrial#884 は register-bit 経路を確認済みです。SLMP #34 は tyaro/banto-industrial#885 の模擬受入れを通過しています。未検証なのは全型・全設定の網羅、実機 I/O、本番配置・資格情報、GUI、#35 診断の実機確認、条件付き範囲の追加証拠、文字列と配布 manifest の製品受入れです。manifest の `capabilities` は認可ではありません。SLMP `Unknown` は確定範囲と混同せず、未確定の機種・wire では安全側に送信を見送ります。V の周期収集除外は意図した現行制限で、修正必須項目ではありません。Modbus register-bit write は FC22 マスク書込みで 1 点ずつ送ります（3xxxx.N は読取り専用）。#35 は診断のみで読書き許可を増やしません。読取り最適化 tyaro/banto-industrial#871 は後回しです。未対応項目の対応順と採否は下の「対応範囲の拡張計画」に従います。

| 段階 | 既存の実装・検証 | 製品段階の未完了条件 |
| --- | --- | --- |
| P0–P1 基盤・契約 | README、AGENTS、CI 判定・ガード、WIT と Hub API の照合 | 後続の互換性判断は変更時に実施 |
| P2 Modbus | コア・WIT ゲスト・独立 Wasmtime 比較、FC1–FC4 read と gated FC5/FC16 write、register-bit read と gated FC22 register-bit write。tyaro/banto-industrial#881 / tyaro/banto-industrial#884 で固定 guest の代表的な adapter・Hub 経路と独立書込みゲートを模擬検証、tyaro/banto-industrial#896 で #44 固定 guest の FC22 を模擬受入れ、tyaro/banto-industrial#905 で固定を同じコードの #47 へ移行 | 未試験の型・設定・経路、実機、本番配置、文字列など採用する製品範囲の検証 |
| P3 SLMP | コア・guest の Q/R 読取りと制御フィクスチャ、条件付き点数、diagnostics-v1。tyaro/banto-industrial#885 で #34 固定 guest と既存 write policy を模擬 Hub 受入れ、tyaro/banto-industrial#887 で #35 固定 guest の診断、tyaro/banto-industrial#895 で #44 固定 guest の範囲外除外を模擬 Hub 受入れ、tyaro/banto-industrial#905 で固定を同じコードの #47 へ移行 | #35 診断の実機確認、未試験の機種・経路・型、実機と本番配置 |
| P4 運用拡張 | 任意 write-v1 とバッチ書込みを実装。tyaro/banto-industrial#881 / tyaro/banto-industrial#884 の Modbus と tyaro/banto-industrial#885 の SLMP で既存 Hub ゲート・監査を模擬確認 | 未試験の書込み対象、実 PLC、本番鍵・配置、UI・配布形式の製品確認 |

指定タグの SLMP 読取り試験命令、機種・経路別範囲拡張、配布管理は将来候補です。文字列は下の拡張計画で採用しました。0101 型式取得と実際の 0401 読取りは異なる証拠であり、成功値は設定範囲全体の証明になりません。Hub が I/O と接続の世代を管理します。これらを採用する場合は別途契約と検証を定めます。

## 対応範囲の拡張計画（2026-10-28 作者決定）

未対応項目への対応順と方針を定める。各段階は個別 PR とし、ゲストを変える PR では Hub 固定版との契約差確認、manifest の版更新、banto-industrial 側の模擬 Hub 受入れまでを 1 単位とする。読取りを書込みより先に、WIT 不変の項目を WIT・Hub 変更を要する項目より先に進める。書込みは既定無効で、ゲストの明示 allowlist と Hub の独立ゲートの両方を要する。実機確認は別途承認を受け、README・CI では模擬確認と実機確認を区別する。

作者の決定:

- FC22 非対応機器は、設定で明示して 4xxxx.N 書込みを送信前に `Rejected` とする。RMW 方式は引き続き不採用。
- SLMP のブロック切替式 R（現在ブロックをゲストから確定できない）と、SD / SW / SM 等の特殊デバイスへの書込みは恒久的に対象外とする。R の代わりに ZR を案内する。
- 文字列、Modbus FC6 / FC15、SLMP 3E はいずれも利用予定があるため採用する。
- SLMP の ASCII コード通信（E3）は採用しない（2026-10-28 作者決定で取消し。当初の採用は文字列の意味と取り違えたもので、通信の ASCII 方式には利用予定がない）。文字列は C1 で扱う。

| 段階 | 対象 | 方針 | WIT / Hub への影響 |
| --- | --- | --- | --- |
| A1 | Modbus コイル／接点だけの機器 | `probe_address` に `0xxxx`（FC1）/ `1xxxx`（FC2）の 1 点を許し、handshake で 1 bit を読む。既定 `40001` は変えない | WIT 不変。Hub 接続テストを FC1/FC2 のみの模擬機器で受入れ |
| A2 | SLMP W のビット読取り（`W1A.0`） | 既存の D ワード内ビット読取りを W に拡張（ワード番号・ビットとも 16 進）。範囲判定は W の点数根拠を使う | WIT 不変 |
| B1 | FC22 非対応機器 | 書込み設定で FC22 不使用を明示した接続では、4xxxx.N 書込みを送信前 `Rejected`。無応答によるバッチ `Timeout` と切断を避ける | WIT 不変 |
| B2 | Modbus FC6 / FC15 | FC6 は 1 ワードの単一書込み（FC16 非対応機器向け、設定で明示選択）。FC15 は連続 coil の一括書込み。どちらも要求全体を覆う allowlist を要する | WIT 不変。Hub で write 経路を模擬受入れ |
| C0 | 文字列の Hub 受入れ確認 | WIT の `tag-spec.string-words` と `value::bytes` を Hub の WASM 経路（保存済みタグ・品質・REST）が扱えるか確認する。未対応なら Hub 側の作業を先行させる | Hub 側の確認・作業 |
| C1 | 文字列（Modbus・SLMP 共通） | Modbus FC3/FC4、SLMP D/W の連続ワードとして読む。ワード内バイト順、NUL 終端、最大ワード数を契約に定める。書込みは Modbus FC16、SLMP 1401 で allowlist が要求全体を覆う場合のみ。長さ超過・埋め方を明記する | C0 に依存 |
| D1 | SLMP Z / LZ / RD / S | 公式表でデバイスコードと単位を確認して読取りのみ追加し、範囲は `Unknown`（1 タグ 1 要求） | WIT 不変 |
| D2 | Q の ZR | 作者決定 A（2026-10-28）: 標準 R の ZR と同様に範囲 `Unknown`・読取り専用・1 タグ 1 要求の 0401 で開く（Q00JCPU・Q00UJCPU・Q25PRH は経路なし）。`q_zr`（SD306–307）は handshake に配線しない | WIT 不変 |
| D3 | Q の拡張 W | 既存 API の配線だけでは成立しない。`q_extension` の `FastWInternal`（SD284–285）は内部 W 点数で、内部＋拡張の総点数ではなく、内部点数を拡張領域へ広げて解釈しない。対象機種・経路への適用条件、内部＋拡張 W 総点数の取得（対象機種での SD310–311 等）、内部点数との区別、必要な経路別アドレス変換、境界試験を別途設計してから実装する。設計と結果は下の「D3 の設計」（2026-10-28 作者決定、ゲスト実装済み） | WIT 不変（設計案では Hub も不変） |
| E1 | SLMP 3E バイナリ | フレームヘッダの変種をコアに追加し、シリアル番号がない場合の応答照合を試験する | WIT 不変。manifest 設定追加 |
| E2 | SLMP 経由接続 | 既存 `Route` のネットワーク番号・局番号等を設定化して検証し、経路ごとに範囲根拠を作り直す | WIT 不変 |
| E3 | SLMP ASCII コード通信 | 不採用（2026-10-28 作者決定）。必要になった場合は、設定 `encoding` で binary と切り替え、WIT の `ascii-hex` 長さ前置きを使う形で改めて段階を起こす | — |
| E4 | SLMP UDP | v1 `transports` は `tcp` のみのため、Hub トランスポートと manifest 契約の変更を先に設計する | Hub・契約の変更 |
| F | SLMP 書込み対象の拡張 | ワード内ビット書込みは原子的手段の有無を公式資料で調査し、なければ RMW を採らず未対応を維持する。TN/CN 等の現在値書込みは同じ接続の点数根拠を設計してから候補とする | 調査後に判断 |
| G | SLMP U/G バッファメモリ、ラベル、FX5 / L / MX / 特殊 CPU | 拡張指定サブコマンド、ラベル用コマンド、機種別の型式・基数・点数根拠をそれぞれ契約設計してから段階的に追加する。Q25PRH と同様に機種別の限定対応とする | 段階ごとに判断 |

C0 の確認結果（2026-10-28、Hub main `1120419` の静的照合）: Hub の WASM 経路は文字列を通せない。タグ登録（`string`、1–128 words、UTF-8 / Shift_JIS）、engine の復号（最初の NUL で切り、不正な符号は項目エラー）、REST の read-now と書込み・監査、native SLMP の文字列は対応済みで、周期収集・履歴・ライブ表示は全ドライバで文字列を扱わない（設計どおり）。不足は WASM アダプタ（`crates/banto-driver-wasm`）だけで、`string` を宣言したゲストの読込み拒否、文字列アドレスの照合拒否、`bytes` 結果のバッチ全体 Fatal、Modbus・SLMP 書込みゲートの数値限定（`string-words` 未設定・`bytes` への変換なし）、Modbus の 1 要求 123 ワードと登録上限 128 ワードの差がある。tyaro/banto-industrial#914 はこの一部を記録している。作者の判断で Hub 側の修正はいったん保留し、C1 はその修正の後に行う。先に WIT・Hub 不変の D1・D2 を進める。

D1 の確認結果（2026-10-28、SLMP 参照マニュアル SH-080956ENG N の公式表）: S は 98H（Q/R、読取り専用、S 対応機種のみ）、Z は CCH（Q）/ 00CCH（R）、RD は 002CH（サブコマンド 0002/0003 のみ＝iQ-R）、いずれも 10 進。ゲストはこの 3 つを範囲 `Unknown` の単独 0401 読取りとして追加し、書込みは持たない。LZ（0062H、ダブルワード）は 0401 一括読出し・1401 一括書込み・0406/1406 ブロックで指定できず、0403 ランダム読出しのダブルワード点だけで読めるため、D1 では追加せずアドレス解析で拒否する。LZ を読むなら 0403 のフレームを新設する別段階になる（採否は未決）。

D2 の確認結果（2026-10-28、作者決定 A）: SD306–307（ZR の点数）は QnU（Q00UJ を除く）だけで、拡張 D または拡張 W を 1K 点以上設定したときだけ値が入る。拡張 D/W は CPU のシリアル先頭 5 桁 09042 以降が必要だが、SLMP ではシリアルを読めず、handshake も拡張点数を読まないため、ゲストは適用条件の証拠を得られない。そこで `q_zr`（`plan_q_zr`/`decode_q_zr_count`）は変更せずゲストからも呼ばず、Q の ZR は標準 R の ZR と同じ方式（0401 サブコマンド 0000、デバイスコード B0H、3 バイトの 16 進番号で 24 ビットに収まること、範囲 `Unknown`、1 タグ 1 要求、書込みなし）で開く。対象は標準 Q のうち Q00JCPU（標準 RAM なし＝ファイルレジスタなし、[QCPU 取扱説明書 SH-080483ENG 印刷 p.147](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=149)）と Q00UJCPU（「ファイルレジスタを持たない」、[QnUCPU 機能解説 SH-080807ENG 印刷 p.392](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=394)）を除いた機種で、Q25PRH は従来どおりファイルレジスタの対応表を持たず経路なし。注意: QnU では ZR の点数を超える ZR 番号が拡張 D/W 領域を指しうる（[QnU pp.402–404](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=404)）ため、ZR の読取りが成功しても ZR の範囲の証明にはならない（読取り専用・範囲 `Unknown` のままなので害はない）。WIT・Hub は変更なし。

D3 の設計（2026-10-28 作者決定: W10000H 未満に限る、診断は変えない、拡張 D は別段階）:

- 公式資料の事実。(1) 拡張 W は内部 W の直後から番号が続き（[QnU 機能解説 SH-080807ENG 4.8 節 印刷 p.402](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=404)）、内部と拡張の間に物理的な連続性はない。プログラムのブロックデータ・インデックス修飾・オートリフレッシュは境界をまたがないよう求められている（同 p.405）。(2) SD310–311 は「内部 W と拡張 W の合計点数」の 32 ビット値（[QCPU 取扱説明書 SH-080483ENG 付録 3 印刷 p.551](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=553)）で、対象は QnU（Q00UJCPU を除く）。拡張 D/W 自体はシリアル先頭 5 桁 09042 以降の CPU が必要（QnU 付録 2）。(3) SLMP・MC では拡張 W も通常の W（デバイスコード B4H、16 進）として「アクセス先 CPU の番号範囲内」で指定する。W10000H 以降へのアクセスに対応しない CPU では ZR に読み替えて指定する（[SLMP 参照マニュアル SH-080956ENG N 印刷 p.38 注 *4](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=40)、[MC 参照マニュアル SH-080008 印刷 p.69](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=71)）。非対応 CPU に 65536 以上を指定すると終了コード 4030H、範囲外は 4031H で拒否される（QCPU 取扱説明書のエラーコード表）。
- 対象: 0101 で標準 Q と分類され、かつ QnU の型式コード（Q00UJCPU 0260H を除く）の CPU だけ。Q00J/Q00/Q01/QnH・Q25PRH・R は変えない。経路は E2 の固定宛先と 4E/3E のどちらでもよく、点数の取得も同じ経路で行う。
- 取得: SD302–303 の読取り（バイト列は不変）の後、対象機種だけ SD310–311 を同じ接続で 1 回だけ読む。PLC の終了コードによる拒否は拡張 W を開かないだけで、接続は続ける（既存の内部 prefix 規則のまま）。データ不足を含むフレーム不正・経路やシリアルの不一致は、既存の SD 読取りと同じく致命的エラーとして切断させる。
- 整合条件: 合計 T（SD310–311）を拡張判定に使うのは、内部 W の点数 WC が SD303 で確定（`Exact`、32768 で飽和していない）し、かつ T ≥ WC のときだけ。T < WC（古い firmware で 0 など）は `Unknown` として使わない。SD303 が飽和（`AtLeast(32768)`）のときは境界 WC が分からないので、拡張は開かず現状どおり先頭 32768 点だけを読む。高速型の SD284–285 で WC を得る案は後の候補とする。
- タグの判定（W の 1・2・4 ワードのタグ全体で判定）: 全体が WC 未満は従来どおり範囲内。全体が [WC, T) にあれば範囲内（拡張 W）として W/B4H で直接読む。WC をまたぐタグは境界越えとして未対応（`Unsupported`）。T 以上に届くタグは範囲外確定（R と同じく設定エラー）。T = WC（拡張なし）の場合は T が W の確定点数になり、WC 以上は範囲外確定になる。
- 要求の組み方: 隣接タグのまとめ読みは WC をまたがない。拡張 W として読むのは W10000H 未満に収まるタグだけ（作者決定）。W10000H 以上に届くタグは、T 未満でも CPU・中継局の対応が 0101 から分からないため未対応（`Unsupported`）とし、T 以上なら範囲外確定のまま。ZR への読み替えは行わない（ZR 点数の SD306–307 は条件付きで、拡張 D 点数も要るため、経路別の変換は持たない）。
- 書込み: D3 は読取りだけ。W の書込みゲートは従来どおり内部 W（SD303）の範囲で判定し、T を使わない。
- 診断（diagnostics-v1）: 変えない（作者決定、案 1）。Q の W は従来どおり内部の範囲で報告し、T は使わない。
- 拡張 D（SD308–309）は別段階（作者決定）。Q の D は内部範囲を超えると範囲 `Unknown` で 1 タグずつ読む現行のままとする。
- 試験: QnU で T > WC の境界（WC−1、WC、WC をまたぐ 2・4 ワード、T−1、T）、T = WC、T < WC、SD310–311 の拒否（継続）と短い応答（切断）、SD303 飽和、Q00UJCPU・非 U の Q・Q25PRH・R で SD310 を読まないこと、W10000H に届くタグの未対応、WC をまたがないまとめ読み、E2 の他局経路と 3E。ゲスト単体と Wasmtime フィクスチャで行い、Hub 模擬受入れは別に記録する。
- 影響: WIT・Hub は不変。SLMP の manifest はこのリポジトリにないため版は上げない。QnU の handshake は 1 往復増える。実機（拡張 W と中継局経由）は未確認として残す。
- D3 の結果（2026-10-28）: 上記のとおりゲストに実装した（コア `plan_q_w_total`/`decode_q_w_total`、handshake の SD310–311 段、`classify_q_w` による判定、まとめ読みの境界分割）。ゲスト単体・Wasmtime フィクスチャ・CI のみで、Hub 模擬受入れと実機は未確認。diagnostics-v1 は作者決定どおり変えないため、読取りでは範囲内として読む拡張 W のタグを、診断では従来どおり内部 prefix 外の `unknown` と報告する。

E1 の結果（2026-10-28）: コアに `Frame`（`E4`／`E3`）を追加し、`Route` の既定は 4E のまま（既存 4E のバイト列は不変）。3E は要求のサブヘッダ `5000` でシリアル番号と予約 2 バイトを持たず、応答は `D000`・経路の折返し・オフセット 7 の長さ（全長＝値＋9、最小 11 バイト）を検査し、4E と 3E の取り違えはフレーム不正とする。ゲストは任意設定 `frame`（`4e` 既定／`3e`）を追加し、Hub の長さ前置き規則（4E は offset 11・adjust 13、3E は 7・9）をコアから得る。3E はシリアル番号で応答を照合できないため、Hub が計画を 1 つずつ計画順に実行することに依拠して発行順で照合し、最古の未応答読取りだけを covers と要求バイトの完全一致で復号する。順序違いの復号、復号済み計画の直後の再利用、未応答の 3E 読取りがある間の `plan` は致命的なプロトコルエラーとして接続を切らせる。ただし 3E は wire 上に識別子がないため、次の周期で同じアドレス・型・covers の要求を作った後は、古い計画と応答の組を新しいものと区別できない。周期をまたぐ一般的な再送防止は保証せず、Hub の逐次実行と失敗時切断に依拠する。ゲスト実装・試験・CI のみで、Hub 模擬受入れと実機確認は未実施。WIT は不変で、既定は 4E のまま。

E2 の結果（2026-10-28、SLMP 参照マニュアル SH-080956ENG N 印刷 pp.20–24）: ゲストに任意設定 `network_no`（10 進 0–239、0 が接続局）、`station_no`（10 進 1–120、`network_no` が 0 以外のとき必須・0 のとき指定不可。自局は FFH 固定）、`target_cpu`（`own`＝03FFH 既定、`cpu1`–`cpu4`＝マルチ CPU 1–4 号機 03E0H–03E3H）を追加し、接続のすべてのフレームの要求先に使う。3 つとも省略すると従来の自局経路でバイト列は不変。接続中に宛先 CPU が変わりうるもの、または別フレームを要するものは送信前の設定エラーとして見送る: 局番号 7CH（局番拡張フレーム）・7DH/7EH（管理局／現在の管理局）、冗長系 03D0H–03D3H、マルチドロップ（I/O 0000H–01FFH、マルチドロップ局番号は 00H 固定）。監視タイマは 0010H（4 秒、自局・他局とも推奨範囲内）のまま。範囲根拠は経路ごとに作り直される: 0101 はアクセス先の型式を返し、SD の点数読出しも同じ固定経路で行い、以後の読書き・診断も同じ経路で組み立てて応答の経路折返しを検査する。接続は経路を変えず、別経路は別接続で handshake からやり直す。接続局が iQ-R 以外で宛先が RCPU のとき 0101 は `RCPU`/0360H を返し、未知の型式として SD・タグを読まない。書込みのゲートは変えない。対象を固定宛先に絞ったのはゲスト段階の判断で、冗長系・管理局指定の採否は必要になった時点で作者が判断する。ゲスト実装・試験・CI のみで、Hub 模擬受入れ・中継局・実機は未確認。WIT 不変。

C1 の規約（2026-10-28 作者決定を含む）: `value::bytes` は文字順の符号化済みバイト列で、長さは常に `2 × string-words`。ゲストは読取り結果を復号・NUL 切りせずに返し、書込みでは Hub が符号化して 0 埋めした全 span を 1 要求で書く（切詰めはしない）。1 ワード内の詰め方は SLMP が下位バイト先（native SLMP と同じ）、Modbus は上位バイト先とする（作者決定）。Modbus の文字列は 123 ワードを上限とする。

実施順は A1・A2 → B1 → B2 → C0 → D1・D2 → E1・E2 → C1 → D3・F・G・E4 とする（E3 は不採用）。各段階の完了は「ゲスト実装・試験・CI 配線」と「Hub 模擬受入れ」を分けて記録する。

## CI の拡張順

現在の `changes` は文書、SDK / WIT、Modbus、SLMP、共通・未知パスを分類する。SDK / WIT は両ドライバに波及し、未知パスと共通設定は全領域に倒す。`scaffold` は必須文書と未知ファイルを確認し、`changes` は変更判定テストを実行する。`modbus-test` は Rust 1.85.1 の整形・Clippy・単体テスト・WASM ターゲットの型検査を実行する。`component-build` は WIT と実コンポーネントを検査し、`component-compare` は同じ成果物を Wasmtime で実行して native と照合する。SDK の area ジョブは製品ソースの誤追加を検出し、`slmp-test` は Rust 1.98.1 の整形・Clippy・単体テスト・WASM ターゲット型検査を行う。`ci-gate` は必須ジョブの failure / cancelled / 想定外の skipped を検出する。

SDK に製品コードを導入する際は、その領域の area ガードを実ビルド・テストに置き換える。SLMP の読取りフレームコアには実テストを追加した。現在の Modbus WASM バイナリは `component-build` で一度作り、比較ジョブに artifact として渡す。Rust キャッシュは main で保存し、PR で復元する。PR の古い実行のみキャンセルする。ジョブ・テストの所要時間を記録し、分割数とキャッシュ設定は実測で決める。nextest の分割はまだ行わない。

ソースや依存を増やす変更で追加の承認が必要なら、その時点で止めて状況を報告する。
