# SLMP: 機種別デバイス範囲とタグ判定の設計案

状態: 設計案（2026-10-25）。実装・実機適合・Hub/WIT 契約の確定を表さない。対象は標準の MELSEC-Q CPU と MELSEC iQ-R CPU / ENCPU。特殊 CPU、別系列、通信経路ごとの対応は機種別に検証する。

## 境界と用語

- WASM ドライバは SLMP フレームの生成・応答解析、0101/0000 の型名・型コード判別、SD からの設定点数取得、タグの範囲判定を担う。ソケット I/O、再接続、接続テストの表示、品質、監査は Hub 側の責務とする。[Hub 側の追跡 Issue #845](https://github.com/tyaro/banto-industrial/issues/845)。
- 「点数」は PLC に設定されたデバイス個数であり、最終アドレスでも CPU バッファメモリの容量でもない。SD は特殊レジスタとしてデバイス読出しする。bit デバイスの点数、word デバイスの点数、MC/SLMP フレーム内の読出し単位（bit / word）を混同しない。X/Y/B/W/SB/SW/ZR などのアドレス表記は十六進、M/D などは十進というデバイス別の基数も区別する。コマンドの点数上限は設定済みデバイス範囲とは別に確認する。型と基数は [MC p.69](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70)、[SLMP pp.37-46](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=38) に従う。
- 型式の公称最大点数を接続先の実設定点数として採用しない。設定点数を取得できない場合は `unknown` とし、範囲外と断定しない。一方、値が取得できても対象機種や経路への適用条件が未確認なら `unknown` を維持する。MC はアクセス先モジュールのデバイス仕様を優先する。[MC pp.69-70](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70)。

## 型式確認と適用対象

読出し専用の `0101/0000` でアクセス先の 16 文字の型名と型コードを取得し、応答の原文字列（空白埋めを含む）と正規化した比較用文字列を保持する。コードと名称の組で対応表を照合する。コードだけで Q の同一コードに複数型名が対応する場合があり、型名だけで判定を完結させない。フレーム形式・経由局を記録する。[MC pp.166-168](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=167)、[SLMP pp.139-140](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=140)。

経由局が iQ-R 系以外の場合、RCPU への 0101 は `RCPU / 0360H` に丸められることがある。この組から個別の R 型式、ENCPU か否か、機種固有の SD 対応範囲を推定しない。再確認手段が決まるまでは必要な型式固有判定を `unknown` にする。[MC p.168](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=169)、[SLMP p.140](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=141)。

以下の表は標準 Q / iQ-R CPU・ENCPU の候補マップである。安全 CPU、プロセス/二重化 CPU、リモートヘッド、L/FX/MX 等に流用しない。型名・型コード・機能版/シリアル・経路と対象 SD の対応が確認できないときは別途機種対応を設計する。

## Q: SD の実設定点数

Q の基本表は各 SD が **1 word の点数**を表す。表の点数をそのままアドレスの数値上限や一括読出し word 数にしない。[Q CPU Appendix 3 pp.549-553](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=551)。

| SD | デバイス | SD | デバイス | SD | デバイス |
| --- | --- | --- | --- | --- | --- |
| 290 | X | 291 | Y | 292 | M |
| 293 | L | 294 | B | 295 | F |
| 296 | SB | 297 | V | 298 | S |
| 299 | T | 300 | ST | 301 | C |
| 302 | D（内部） | 303 | W（内部） | 304 | SW |

`SD292/294/302/303` の旧欄は設定点数が 32769 以上なら `32768 (8000H)` を返す。これを厳密な末尾と解釈しない。飽和した欄しか取得できない場合、32768 未満についてのみ確定し、それ以上は `unknown` とする。[Q CPU p.550](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=552)。

Universal Q の拡張 M/B は `SD286-287` / `SD288-289` の low/high 32-bit 点数を使えるが、表の注記 *6 に従い serial 先頭 5 桁が `10042` 以降の対象機に限る。高速 Universal QnUDV/QnUDPV の内部 D/W は `SD282-283` / `SD284-285` の low/high 32-bit 点数。適用条件を確認せず旧欄と置き換えない。[Q CPU pp.549,553](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=551)。

QnU の D 合計（内部 + 拡張）は `SD308-309`、W 合計は `SD310-311`、ZR は `SD306-307` の low/high 32-bit 点数。これらの表の注記 *7 は Q00UJCPU を除外する。Q00UJCPU にこの合計マップを適用しない。内部の `SD302/303`、高速型の `SD282-285`、合計の `SD308-311` は意味が異なる。[Q CPU pp.550-553](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=552)、[QnU pp.403-405](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=404)。

Q の拡張 D/W は内部領域の次へ**論理デバイス番号が連続**するが、物理領域は連続しない。経由モジュールの対応可否を別に確認し、未対応の経路で必要な場合はマニュアルに従い ZR 番号へ変換する。この変換と一括読出しの境界は経路別に検証する。QnU p.405 の内部/拡張境界を跨がせない注意は、PLC の自動リフレッシュ・ブロックデータ命令等の指定に関する記述であり、**MC のすべての読出しを一律禁止する根拠にはしない**。[QnU pp.403,405-407](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=404)、[MC p.69](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70)、[SLMP p.39](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=40)。

## iQ-R / ENCPU: SD の実設定点数

以下は各先頭 SD が low word、次の SD が high word の **32-bit 点数**を表す。Z/LZ だけは単一 word。型式・ファームウェア・通信経路の対応が判明した標準 CPU にのみ用いる。[iQ-R CPU Appendix 5 pp.616-617](https://www.mitsubishielectric.com/dl/fa/document/manual/plc/sh082488eng/sh082488engf.pdf#page=618)。

| bit 系 | 先頭 SD | bit 系 | 先頭 SD | word / その他 | 先頭 SD |
| --- | ---: | --- | ---: | --- | ---: |
| X | 260 | Y | 262 | D | 280 |
| M | 264 | B | 266 | W | 282 |
| SB | 268 | F | 270 | SW | 284 |
| V | 272 | L | 274 | Z（単 word） | 300 |
| S | 276 | T | 288 | LZ（単 word） | 302 |
| ST | 290 | C | 292 | ZR | 306 |
| LT | 294 | LST | 296 | RD | 308 |
| LC | 298 |  |  |  |  |

T/ST/C/LT/LST/LC の設定点数と、接点・コイル・現在値の SLMP デバイスコード/bit-word-double-word 単位は別の情報である。存在しないデバイスコードへ一般化しない。[MC p.69](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70)、[SLMP pp.37-39](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=38)。

## 接続テストとタグ判定の設計案

1. PLC 設定待ちの保存は許す。保存時は型・設定書式・アドレス表記・長さの不正を検出するが、未接続というだけで型式別の点数を推測しない。「形式/型が不正」と「実機未確認」を別状態にする。
2. 本番利用前の**読出し専用**接続テストで 0101、機種別に適用可能な SD、対象タグの読出し可否を確認する。実機との通信は本設計文書では行っていない。実機への write 試験は別承認とする。Hub が I/O と結果表示、品質、監査を扱う。
3. WASM はタグの開始点と全占有点数を、オーバーフローを避けて半開区間 `[start, end)` で計算する。設定点数 `N` の `[0, N)` に**完全に収まるタグだけ**を読出し候補とする。U32 の 2 word、文字列の全 word、bit と word の変換単位を含め、途中で切ったタグを読まない。成立する候補をフレーム制限・経路の境界に従って分割する。部分的な旧値を新しい正常値として扱わない。
4. 判定は少なくとも `in_range` / `out_of_range` / `unknown` と取得世代を区別する。範囲外が確定したタグと、型式・SD・経路の情報不足による `unknown` を混ぜない。接続テストや通常読出しの失敗時に前回の正常値を「現在正常」として表示しない。unknown 時に運用上どの操作を許すかは未決。
5. 点数キャッシュは接続先、0101 の生応答と判別結果、取得世代、経路、接続セッションに結び付ける。接続先変更・設定変更・再接続・再取得で失効し、古い点数を新しい接続へ持ち越さない。
6. 将来の書込みは命令に含まれる**全範囲**を送信前に検査し、一部だけを送らない方針。実書込み、複数タグ/バッチの原子性と検査単位、WIT の拡張は後続設計と承認事項であり、現行実装の能力とはしない。

## 出典と未検証事項

- [QCPU User's Manual (Hardware Design, Maintenance and Inspection), Appendix 3 pp.549-553](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf) — SD282-311 と適用注記。上記の飽和・serial・Q00UJ 除外は表で確認した事実。
- [QnUCPU User's Manual (Function Explanation, Program Fundamentals), pp.400, 402-406](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf) — ファイルレジスタと拡張 D/W の配置・境界。ZR 変換の具体式は経路ごとに追加確認が必要。
- [MELSEC Communication Protocol Reference Manual, pp.68-69, 86, 166-168](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf) — デバイスコード/基数、コマンド、型名・コードと経由局。
- [MELSEC SLMP Reference Manual, pp.34-46, 138-140](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf) — フレーム、サブコマンド、デバイスアクセス、0101 応答。
- 本書のタグ状態、キャッシュ、接続テスト、write の事前検査は**設計案**。接続経路ごとの SD 読出し可否、ZR 変換、実機 read の結果、例外/品質の Hub 表示、WIT 変更、実書込みは**未検証・未実装**。
