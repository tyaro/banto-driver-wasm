# SLMP: 現行ゲスト能力、範囲の証拠と将来候補

As of merged main `b3d944f` (#35), the separate SLMP guest in [slmp-component](../../slmp-component/src/lib.rs) connects this I/O-free core to the existing WIT connection handshake and decimal D tags or hexadecimal W tags of types `i16/u16/i32/u32/i64/u64/f32/f64`. The R handshake reads SD280-283 together and keeps the complete D and W counts separate. On a recognized standard Q CPU, one same-connection SD302-303 read obtains separate **internal** D and W counts. An exact Q25PRHCPU/004CH pair also reads SD302-303; internal W reads within the SD303 guaranteed prefix are supported, while general D reads retain unknown range. A Q W tag is read only when its entire one-, two-, or four-word span is inside the SD303 guaranteed prefix; an unknown, refused, or insufficient count leaves it unsupported. This rule does not cover Q extended W. The old SD303 value 32768 guarantees only the first 32768 points. Verified adjacent tags are batched in input order up to the core's 960-word 0401 limit; unknown-range tags, gaps and duplicates break runs. Ordered `covers` map each complete value or whole-batch PLC refusal back to its tag. [Wasmtime fixtures](../../slmp-runtime/src/lib.rs) use controlled Q/R responses. [QCPU Appendix 3, printed pp.549-553](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=551) defines the SD303 point count. The [discovery contract proposal](../../docs/SLMP_DISCOVERY_CONTRACT.md) does not change WIT or Hub. No PLC communication was performed.

M (decimal) and X/Y/B (hexadecimal) are read as device-point `bit` tags through 0401 bit-unit requests. Each response bit becomes WIT `bool`; bit tags cannot use a word bit suffix, numeric type, or `string_words`. Read planning still treats bit-area configured ranges as **Unknown**, including on a recognized Q25PRHCPU/004CH. An explicit bit tag therefore gets one request per tag; a successful response never upgrades the range evidence. The separate local write stage below obtains M/Y/L/B counts when an enabled common write policy names a bit area. The core limits a bit request to 7168 points and rejects malformed packed nibbles or nonzero odd-point padding. [Mitsubishi MC device codes and radices, printed pp.68-69](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70) and [tyaro/slmp at 40a2f1d](https://github.com/tyaro/slmp/blob/40a2f1d9cd4e7eb641423284e0678e81200b91a6/src/commands/device_access/read/bulk.rs) agree on Q/R bit subcommands and device layout. The latter decodes only the low bit of each nibble and serializes the R high address byte as zero; this core retains its stricter malformed-nibble check and full 32-bit R field from the official protocol. No code or dependency was copied from that library.

The guest accepts L/F/SM (decimal) and SB (hexadecimal) as `bit`, plus SD (decimal) and SW (hexadecimal) as the existing numeric types. The corresponding Q/R device codes are 92/93/91/A1 and A9/B5. SLMP also defines a global V edge-relay read (code 94), but V is excluded from collection tag registration and read planning: periodic polling can miss its one-scan edge. SD and SW used as tags are separate from the handshake's SD registers: no count for these new tag areas is inferred from the D/W count response. Their ranges remain **Unknown** and each explicit tag is read alone; a successful read does not verify the configured range. The #34 V exclusion and #35 diagnostics add no write permission. `S` (R SFC/firmware condition), Q extended W, Q ZR and block-switched R still need separate model/route evidence; supported timer/counter variants, standard R ZR and D word-bit reads are described below. The [official MC code table, printed p.68](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70) and [tyaro/slmp device table at 4500aea](https://github.com/tyaro/slmp/blob/4500aea4e1cdf072e41eb844d6a4caf4d550feb8/src/device.rs) agree on the codes and number bases; the official table governs series-specific subcommands and full R address width. No library code was copied.

### Merged optional diagnostics for Q X and R SB/F (#35)

For an exact currently recognized StandardQ name/code pair, diagnostics-v1 reports the unsigned 16-bit X point count from SD290. For an exact currently recognized StandardR pair, it reports SB from SD268-269 and F from SD270-271 as unsigned 32-bit Low/High counts, independent of the tag `word_order` setting. These words are slices of the existing Q SD290-294 and R SD262-275 requests. A diagnostic call for one of these areas starts that one bounded request when needed, or reuses same-connection handshake evidence when it was already acquired. X and SB tag addresses are hexadecimal; F addresses are decimal. For an exact count N the inclusive last point is N-1, and zero means no configured points. Q X=32768 remains exact; the older M/B saturation rule does not apply.

Other model families, Q25PRH, R X, Q SB/F, and V receive no new range evidence. A refusal or unavailable SD data remains Unknown; a malformed response is fatal for that connection. The count is a configured boundary for diagnostics, not a guarantee that a physical device read succeeds or that periodic polling captures a signal. Read/write permissions, the write allowlist, and Host operational controls do not change. The #35 Q X/R SB/F diagnostics have no Hub acceptance yet; #885 covers the #34 guest in a simulated Hub suite. Physical PLC and production behavior remain unverified. [QCPU Appendix 3, printed p.550](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=552); [iQ-R point count table, printed p.616](https://www.mitsubishielectric.com/dl/fa/document/manual/plc/sh082488eng/sh082488engf.pdf#page=618).

## Common SLMP write policy and compatibility

### Guest typed-word and bit-write policy (simulated Hub acceptance for #34)

The same saved write mode and area-qualified allowlist now also describe numeric `i16/u16/i32/u32/i64/u64/f32/f64` D/W tags and `bit` M/Y/L/B tags. This merged guest/core capability is **not** a Host authorization or PLC test. Old D/W intervals retain their meaning and grant no bit access. M/L addresses are decimal; Y/B are hexadecimal. Each explicitly named bit interval contains at most 7168 points, and a word interval at most 960 words. A whole request must use one area, ascend without holes or overlaps, fit one allowed interval and fit the complete same-connection SD count. Q25PRH bit and W writes, X/input, special bit areas, timer/counter writes, word-bit read/modify/write, and block-switched R remain disabled. D3000/D3001 remain protected.

The guest first reads D/W counts. On the write-handshake path, an enabled allowlist naming a bit area and an exact standard Q/R model also request Q SD290-294 or R SD262-275 on that connection. Optional diagnostics-v1 can request the same bounded SD block independently of write enablement, or reuse same-connection evidence. M/Y/L/B point counts serve both write preflight and diagnostics-v1 range assessment; a refusal leaves them Unknown and forbids bit writes. A diagnostic verdict never grants a write. Q's old M/B value 32768 proves a prefix, not an exact endpoint. The bit wire frame uses 1401/0001 for Q or 1401/0003 for R, with two Bool points per byte and zero padding in an odd final nibble. Numeric values use full 1/2/4-word widths and the configured word order. Exact WIT Int/Uint values preserve 64-bit integers; legacy Float integers above 2^53-1, nonfinite float writes, and f32 overflow are rejected. A valid empty ACK or PLC refusal maps to every original tag; an uncertain outcome is never replayed automatically. [MC batch write, printed pp.95-96](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=97).

The earlier Host boundary parsed saved settings and checked `operational_write_item` for D/W u16 only. Industrial #885 then accepted the #34 guest in a simulated Hub suite, including existing common allowlist, independent preflight, authorization and audit regression. That validation does not cover every numeric type or a physical write; the WIT files are unchanged.

The saved `d_u16_write_mode` and `d_u16_write_allowlist` keys remain compatible, but the guest presents them as one SLMP write enable and one area-qualified range list. Existing `D3010-D3013` continues to allow only D. Adding `W1A-W1F` in the same list explicitly allows that W interval; there is no W-specific switch. D numbers are decimal, W numbers hexadecimal. Intervals must stay in one area, be ascending and nonoverlapping within that area, and each contain at most 960 words. The entire request is validated before one 1401 word-unit frame. D3000/D3001 remain protected.

For the earlier D/W u16 extension, only confirmed standard R D/W and standard Q internal W spans use the W path with same-connection SD counts; exact Q25PRH D/u16 rules remain intact. The later typed-word/bit policy above adds eligible numeric D/W and Bool M/Y/L/B writes. Q25PRH W/bit, unknown ranges, word-bit writes and other unlisted types remain rejected. A valid empty ACK completes all covered items; uncertain ACK/timeout must not be resent automatically. The Host boundary recorded for this earlier stage validated **D/u16 only** in its saved-policy parser and `operational_write_item`. The later #885 simulated suite checks the current common range grammar and separate Hub gate for the #34 guest, without certifying every value type, physical write or production enablement. WIT and the separate Q25PRH test path are unchanged.

## Remaining Q/R read devices (merged guest, controlled fixtures)

This guest accepts the following explicit read tags on a recognized standard Q or iQ-R CPU. Timer/counter addresses are **decimal device points**, while `ZR` addresses are **hexadecimal file-register words**. Each tag is read alone because the current same-connection SD handshake obtains only D/W counts; neither 0101 nor a successful 0401 reply verifies these new configured ranges. No Q25PRH timer or file-register map is inferred. The [MC device table, printed pp.68-69](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70) defines the codes and units; [tyaro/slmp rev 4500aea](https://github.com/tyaro/slmp/tree/4500aea4e1cdf072e41eb844d6a4caf4d550feb8) was checked for common Q device codes without importing code.

| CPU | Bit tags, 0401 bit units | Word/current tags, 0401 word units |
| --- | --- | --- |
| Standard Q | `TS`, `TC`, `SS`, `SC`, `CS`, `CC` | `TN`, `SN`, `CN` (`i16`/`u16`) |
| Standard R | `TS`, `TC`, `STS`, `STC`, `CS`, `CC`, `LCS`, `LCC`; `LTS/LTC/LSTS/LSTC` via current-value word flags | `TN`, `STN`, `CN` (`i16`/`u16`); `LTN`, `LSTN`, `LCN` (`i32`); `ZR` (numeric 1/2/4-word types) |

R long timer/retentive timer current tags send **four word access points for one logical timer** and decode only the first two current-value words. The third word has coil bit 0 and contact bit 1; `LTC/LTS` and `LSTC/LSTS` read those bits from the corresponding `LTN/LSTN` four-word response. They do not put a flag device at the head of a batch read. The fourth word is system data. Long counter current tags use two words. The manual also permits long counter contacts/coils through bit-unit batch reads, and this stage enables R-only `LCS/LCC` with 0401/0003 bit-unit requests. [MC batch read, printed pp.69, 86-91](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=88). `R` block-switching file registers remain unsupported because a register number alone does not identify the block and some program-scoped file registers cannot be read externally. Q `ZR` remains unsupported because its SD306-307 evidence is conditional on extended D/W configuration; this guest has no such applicability evidence. Standard R `ZR` uses serial-number access with hexadecimal notation and an unknown range, not an invented bank or count.

`D10.F/bit` denotes bit F of the **single D10 word**, not a bit-unit D request. The guest sends 0401 word units for D10 and extracts bit 15 locally; `address.parse.bit=15`, `index=10`, `word-count=1`, and write capability remains false. Tag offsets move by bit (`D10.F + 1 = D11.0`); `offset-words` moves by word (`D10.F + 1 word = D11.F`). Invalid suffixes and incomplete word responses are rejected. D's existing known/unknown range rules apply to the entire underlying word. [Mitsubishi iQ-R MES manual, word-bit notation](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh081423eng/sh081423engq.pdf).

This stage changes no WIT, Hub, D/u16 write gate, gap optimization, or real PLC configuration. Timer/counter and file-register counts are still unconfirmed on a live CPU; the normal unknown-range one-tag read policy applies. `S` remains gated by SFC/firmware evidence. Exact `Q25PRHCPU/004CH` now reads SD302–303 on the same connection and permits internal W tags only when the whole tag fits the SD303 guaranteed prefix. General Q25 D stays range-Unknown; extended W, conditional Q ZR, and block-switched `R` remain unsupported.

The WIT `word-count` for a long timer `i32` remains two for the typed value; the additional response words are protocol status, not tag width.

## Optional contiguous D/u16 write prototype

The original [read-only `driver` world](../../wit/driver.wit) is unchanged. The separate [versioned write extension](../../wit/write-v1.wit) adds `driver-with-write-v1` and `write-v1.write-next`. A host must explicitly negotiate that world and pass the **same** `session.connection` resource used for 0101 discovery and SD reads. Older guests still satisfy the original world. This is a controlled guest prototype; the current Hub does not enable it for production.

This local extension produces one binary 4E `1401/0002` frame for 1–960 consecutive, ascending D/u16 tags on a confirmed StandardR model. The **entire span** must be inside the configured SD280-281 D range. Every item must have a finite, integral `float(f64)` value from 0 through 65535. In this earlier StandardR-only path, unknown range, Q and Q25PRH, other tag kinds, gaps, duplicates, reversed addresses, and invalid values reject the entire request before a frame is produced. A successful zero-payload ACK completes every covered item; a nonzero end code refuses every covered item. A malformed or mismatched ACK is fatal and requires a new connection. After a frame is handed to the host, timeout or disconnection leaves the physical write result **unknown**. Neither guest nor host should automatically resend or roll back it. The old single-tag behavior remains valid. The existing WIT is unchanged; a Host must independently enforce its saved allowlist and write gate; production writing is not enabled by this guest change. The four-word D3010–D3013 loopback is a local simulation only and does not authorize or establish PLC range-write safety. The trust boundary is the signed guest; the sandbox does not verify the meaning of arbitrary guest commands. [SLMP Reference Manual, printed pp.51-53](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=53).

## Local Q25PRH four-word write simulation

The Q25PRH test path is enabled only by an explicit `q25prh_d3010_3013_test_write=enabled` connection setting. The advanced settings schema declares this as a **test-only** choice, defaulting to `disabled`; saving it does not open the production Host write gate. It requires the exact 0101 `Q25PRHCPU / 004CH` pair, a same-connection SD302-303 two-word reply whose SD302 value guarantees the full internal D3010..D3013 span, and exactly four ascending `D3010` through `D3013` `u16` items. The old SD302 value 32768 is a guaranteed prefix, not an exact endpoint. An SD refusal, missing or malformed reply, count below 3014, or invalid item rejects the whole test write without sending a frame. Once all conditions hold, the guest constructs one Q binary 4E `1401/0000` frame with a 24-bit D address and count four. The general Q read-range verdict remains unknown; SD308-311 are not borrowed from other Q models. The existing StandardR write gate is unchanged. A valid empty ACK completes all four items, while a malformed or uncertain ACK must not trigger automatic replay. The production Hub does not enable this test path. The mock Wasmtime fixture uses no PLC connection; the Q PLC port has not been verified in local project records. The WIT files are unchanged. [Mitsubishi technical bulletin, PDF p.22](https://www.mitsubishielectric.com/fa/document/technews/plc/fa-a-0214/faa0214a.pdf#page=22) assigns SD290-304 to QnPRH; [QCPU Appendix 3, printed pp.549-550](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=551) identifies SD302 and its saturation behavior.

## Normal D/u16 write policy (local guest stage)

The normal write path is a separate, disabled-by-default connection setting: `d_u16_write_mode=enabled` plus a nonempty `d_u16_write_allowlist`. The list uses comma-separated, ascending, nonoverlapping decimal ranges such as `D3010-D3013,D4000-D4009`, with no spaces. It is limited to 256 ASCII characters, 16 ranges, 1–960 words per range, and 24-bit device numbers. D3000 and D3001 are protected, including when a range crosses them. Each request must be 1–960 consecutive ascending D/u16 items and fit completely inside **one** allowed range. Missing, disabled, malformed, or empty enabled policy prevents a normal write frame.

The guest additionally requires the same connection's exact StandardR model and SD280–281 D count, or the exact `Q25PRHCPU / 004CH` pair and SD302 internal D prefix. Unknown or insufficient evidence rejects the entire request before serial allocation. R uses `1401/0002`; Q uses `1401/0000` with a 24-bit D address. The separate Q four-point test flag above remains limited to D3010–D3013 and cannot be enabled together with normal mode. ACK refusal maps to every item; malformed or uncertain ACK cannot be automatically retried. WIT remains unchanged. This local guest implementation does not configure a Host production write gate or perform PLC communication.

## Current read policy and operator responsibility

The guest checks a **whole tag** against the configured range only when 0101 identifies a supported standard Q/R name and code, the applicable SD response is usable, and the count's scope and unit match the tag. Q SD302 is **internal D only**: a tag wholly below its reported count is verified in-range; a tag reaching or crossing the count is **unknown** because an extended D area may exist. Q SD303 is **internal W only**: this stage reads W tags within its guaranteed prefix and does not infer an extended W endpoint. If an old Q count is saturated at 32768, only the guaranteed prefix is verified. R SD280-281 gives a complete D count and SD282-283 gives a complete W count: a tag reaching or crossing its area's exact count is left uncovered. An SD refusal leaves the count unknown. Verified adjacent tags may share a request, while known and unknown D tags never share a frame; Q W with unknown count is left uncovered. `word_order` selects `low_high` (default) or `high_low` within each multiword value. Unsupported tags are left uncovered for the host's individual `Unsupported` result. The guest returns typed WIT `int/uint/float` values; downstream `f64` projection can lose integer precision above `2^53`.

When the model is recognized but the D range is unknown, an explicit valid tag can be read at the **user's responsibility**. Check the actual PLC device settings and route before relying on its value. Successful reading does **not** verify that the address is within the configured range, and it does not change the unknown status. An unrecognized model/name-code pair or a 0101 refusal does not establish whether Q or R 0401 framing applies (their subcommands and device-number widths differ), so this guest does not send an SD or tag read in those cases. That limitation is distinct from an unknown **range** with a known Q/R wire series. Invalid address or type and unsupported wire/route meaning remain errors. The original read-only `driver` world returns values and item errors without a structured range verdict. The separate optional `diagnostics-v1` world exposes count bounds and per-tag `Inside` / `Outside` / `Unknown` / `Invalid` / `Unsupported` through `diagnose-next` on the same connection. `Inside` requires a proven span and a supported guest read path; `Outside` requires an exact configured count. A successful ordinary read does not change Unknown to Inside. Host display, quality and connection-generation handling still require their own integration, and diagnostics never expands read/write permission.

The exact `Q25PRHCPU / 004CH` 0101 name/code pair establishes Q word-read wire format. Mitsubishi's QnPRH device-assignment table includes SD290-304, and SD302 gives one word of configured internal D points. The guest requests SD302-303 on the same connection after this exact pair. General explicit numeric D reads remain individual with an `Unknown` range verdict, even when SD302 succeeds; SD302 also supports the gated four-word test write above. Internal W reads are allowed only inside the SD303 guaranteed prefix, without inferring an extended W endpoint. A successful read does not verify a full D range. A different name, code, or 0101 refusal does not gain this exception. [SLMP model-code table](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=141), [QnPRH device assignment](https://www.mitsubishielectric.com/fa/document/technews/plc/fa-a-0214/faa0214a.pdf#page=22).

0101 obtains a model name/code; it cannot validate every point. A later optional test could try each **registered tag** individually, but it would not prove the entire configured range. No full address scan or PLC connection is part of these fixtures.

## Conditional Q extension fields (current core stage)

[QCPU Appendix 3 printed p.549](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=551) defines low/high SD286-287 for the QnU complete M count and SD288-289 for the complete B count. Its *6 footnote (printed p.553) requires CPU serial first five digits 10042 or later; it does not carry the separate Q00UJ exclusion used for ZR. [QnU Appendix 2 printed p.467](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=469) distinguishes bit-device extension version/serial support from extended D/W support. The core requires independently confirmed version, serial, and route evidence before planning these reads.

The same QCPU table assigns SD282-283 to high-speed QnUDV/QnUDPV internal D points and SD284-285 to internal W points. The current classifier recognizes only five standard QnUDV name/code pairs, so this stage plans the pairs only for those five. QnUDPV remains unsupported. The SD282-285 entries have no additional serial footnote; model/version/route evidence is still required by this core. A complete two-word response yields the named count: M/B is the full configured count; fast D/W is internal-only. The QCPU table's 32K wording is not used as a blanket rejection threshold; QnU model-specific internal D capacities can exceed it. A nonzero high-word fixture verifies decoding and does not establish a possible physical capacity.

SD286-289 are complete M/B counts and must not be added to old saturated M/B words SD292/294. SD282-285 are internal D/W counts, distinct from old SD302/303 and D/W total fields SD308-311. Q ZR SD306-307 is separate. The core does not synthesize D/W totals or claim an exact tag range. Missing applicability evidence, unsupported model, or partial SD response yields `Unknown`. Connection generation binding and cache invalidation remain future Hub/driver integration work. No PLC communication was performed here; the limited write frame is described above.

以下の I/O 非依存コア4段階は、追加当時の実装範囲を記録する。現在の WIT ゲストと模擬 Hub 受入れは冒頭を参照。

## 旧段階の実装記録（QnU ZR の条件付き点数）

[`src/q_zr.rs`](src/q_zr.rs) は標準 Universal QnU（Q00UJCPU を除く）に対し、拡張 D **または**拡張 W を 1K 点以上設定したこと、対象 firmware と通信経路の適用を別途確認できた場合だけ `SD306–307` の low/high pair を ZR の点数として計画・復号する。条件未確認・条件外・読出し失敗・片 word だけの場合は `Unknown` であり、ゼロや厳密な上限を捏造しない。[Q CPU Appendix 3 p.550 の SD306–307 欄と *7](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=552)。この core は条件の証拠を収集せず、I/O と cache も持たない。証拠と SD 値は同じ接続先・取得世代に紐づけ、再接続時に失効させる必要がある。[独立フィクスチャ](tests/q_zr.rs) は Q00UJ 除外、条件分岐、2 word 順、0401 要求を検査する。

`SD647` は選択中ファイルの容量を 1K word 単位で端数切捨てし、拡張 D/W の使用時は ZR との共有容量を表す。このため `SD647 × 1024` を ZR 単独の厳密な点数上限にしない。[QnU p.400](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=402)、[pp.403–404](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=405)。拡張 M/B、高速 D/W、Q の全 D/W 点数と SD308–311 の機種別解釈は本工程では扱わず `Unknown` に留める。

## 旧段階の実装記録（基本 SD 点数）

[`src/sd_basic.rs`](src/sd_basic.rs) は I/O 非依存の取得計画と復号を追加した。標準 Q の `SD290–304` は単 word、標準 iQ-R / ENCPU の下表は原則 low/high の 2 word、Z/LZ だけ単 word として計画する。返す数値は**設定点数**であり、アドレス末尾や SLMP 0401 の読出し点数上限ではない。Q の `DInternal` / `WInternal` は SD302/303 の内部点数だけを表し、Q の全 D/W を要求した場合は `Unknown` に留める。Q の旧 M/B/内部 D/内部 W 欄が 32768 を返した場合は `AtLeast(32768)` とし、厳密な末尾を確定しない。型式が不明、読出しが未取得、取得 word 数が違う、経路の対応が未確認の場合も `Unknown` を返す。[Q CPU Appendix 3 pp.549–553](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=551)、[iQ-R Appendix 5 pp.616–617](https://www.mitsubishielectric.com/dl/fa/document/manual/plc/sh082488eng/sh082488engf.pdf#page=618)。

R の S 点数は R00/R01/R02CPU または Rn(EN)CPU の確認済み firmware 12 以降でのみ数値を確定する。これ以外は `Unknown`。経路確認・firmware 版の取得自体はこの core にない。取得した SD 値・型式・経路の証拠は接続先と取得世代に紐づけ、再接続時には失効させる必要がある。この段階は cache を持たない。基本 SD API では Q の ZR / 全 D/W を `Unknown` に保ち、ZR のみ上記の条件付き API で扱う。native にこれら SD 点数の取得実装はなく、独立フィクスチャは公式表と既存の 0401/0101 フレームコアを照合する。[独立フィクスチャ](tests/sd_basic.rs)。

## 旧段階の実装記録（0101 型式名読取り）

[`src/type_name.rs`](src/type_name.rs) は I/O のない binary 4E `0101/0000` 要求を生成し、正常応答の 16 バイト ASCII 型名と little-endian 2 バイト型式コードを解析する。原 16 バイト（空白埋めを含む）と末尾空白を除いた表示名を両方保持する。正式な表にある標準 Q と iQ-R CPU / ENCPU の**型名と型式コードの組**だけを分類し、未知または不一致の組と `RCPU / 0360H` は `Unknown` を返す。不正な文字列、応答長、経路・シリアル、非ゼロ終了コードは区別する。[SLMP Reference Manual pp.138–141](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=139)。[独立フィクスチャ](tests/type_name.rs) は公式 Q02UCPU 例と R04ENCPU、未知型式・破損・拒否を検査する。native の SLMP には現時点で 0101 処理がなく、共通の 4E ヘッダー・応答検査のみ静的照合した。

この分類は候補の識別に限る。機種・経路・ファームウェア固有の SD 対応と PLC の実設定点数は取得しておらず、タグ範囲を確定しない。特殊 CPU のコードや類似名へ標準表を流用しない。この時点の型式名コア単体には WIT/Hub 接続、実機 I/O、書込みを含めていなかった。後続のゲストと模擬 Hub 結合は冒頭に記載した。

## 旧段階の実装記録（0401 フレームコア）

[`src/lib.rs`](src/lib.rs) は標準 Q / iQ-R の binary 4E フレームによる `0401` 連続読取り要求と応答を扱う、I/O のない `no_std` コア。M/X の bit と D/W/SD の word、Q の `0001/0000` と R の `0003/0002` サブコマンド、Q の3バイト・R の4バイトデバイス番号、little-endian の点数・word 値を実装した。デバイス番号の数値は呼出側がアドレス基数に従って解析済みとする。1要求の上限は bit 7168点、word 960点。これは SLMP の命令上限であり、機種固有の実設定範囲を保証しない。[SLMP Reference Manual pp.20–31, 37–47](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=21)。

応答はシリアル・経路・長さ・データ点数を検査し、機器の非ゼロ終了コードをフレーム不正と区別する。bit の各 nibble は仕様上の 0/1 のみ許し、奇数点の最後の未使用 nibble は 0 を要求する。native の bit 復号は nibble の最下位 bit だけを見るため、この不正データの受理挙動は意図的に模倣しない。[SLMP Reference Manual pp.26–30, 41–43](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=27)。[独立フィクスチャ](tests/read_frame.rs) は native `banto-driver-slmp/src/frame.rs` の正常な wire layout と照合した。R は公式の4バイト番号全幅をフレーム化する一方、native の現在のアドレス上限 `0xFFFFFF` とは差がある。実機・対象 CPU の範囲は後続で判定する。

この 0401 フレームコア単体は SD 実設定点数、指定タグの読取りテスト、WIT コンポーネント、Hub I/O、実機通信、書込みを含まない。後続に実装されたゲストと模擬 Hub 結合は冒頭に記載した。以下は当時の機種別範囲設計案である。

状態: 設計案（2026-10-25）。実装・実機適合・Hub/WIT 契約の確定を表さない。対象は標準の MELSEC-Q CPU と MELSEC iQ-R CPU / ENCPU。特殊 CPU、別系列、通信経路ごとの対応は機種別に検証する。

## 境界と用語

- WASM ドライバは SLMP フレームの生成・応答解析、0101/0000 の型名・型コード判別、SD からの設定点数取得、タグの範囲判定を担う。ソケット I/O、再接続、接続テストの表示、品質、監査は Hub 側の責務とする。[Hub 側の追跡 Issue #845](https://github.com/tyaro/banto-industrial/issues/845)。
- 「点数」は PLC に設定されたデバイス個数であり、最終アドレスでも CPU バッファメモリの容量でもない。SD は特殊レジスタとしてデバイス読出しする。bit デバイスの点数、word デバイスの点数、MC/SLMP フレーム内の読出し単位（bit / word）を混同しない。X/Y/B/W/SB/SW/ZR などのアドレス表記は十六進、M/D などは十進というデバイス別の基数も区別する。コマンドの点数上限は設定済みデバイス範囲とは別に確認する。型と基数は [MC p.68](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70)、[SLMP pp.37-46](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=38) に従う。
- 型式の公称最大点数を接続先の実設定点数として採用しない。設定点数を取得できない場合は `unknown` とし、範囲外と断定しない。一方、値が取得できても対象機種や経路への適用条件が未確認なら `unknown` を維持する。MC はアクセス先モジュールのデバイス仕様を優先する。[MC pp.69-70](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70)。

## 型式確認と適用対象

読出し専用の `0101/0000` でアクセス先の 16 文字の型名と型コードを取得し、応答の原文字列（空白埋めを含む）と正規化した比較用文字列を保持する。コードと名称の組で対応表を照合する。コードだけで Q の同一コードに複数型名が対応する場合があり、型名だけで判定を完結させない。フレーム形式・経由局を記録する。[MC pp.166-168](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=167)、[SLMP pp.139-140](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=140)。

経由局が iQ-R 系以外の場合、RCPU への 0101 は `RCPU / 0360H` に丸められることがある。この組から個別の R 型式、ENCPU か否か、機種固有の SD 対応範囲を推定しない。再確認手段が決まるまでは必要な型式固有判定を `unknown` にする。[MC p.167](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=169)、[SLMP p.140](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=141)。

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

QnU の D 合計（内部 + 拡張）は `SD308-309`、W 合計は `SD310-311` の low/high 32-bit 点数。**QCPU の詳細表は、両組とも内部領域と拡張領域の合計と明記する**。`SD306-307` は ZR の low/high 32-bit 点数だが、**拡張 D または拡張 W のいずれかを 1K 点以上設定した場合にのみ**点数が格納される。条件外・適用条件不明・取得不能なら ZR の厳密な点数は `unknown` とし、SD306-307 の値を無条件に上限として採用しない。これらの表の注記 *7 は Q00UJCPU を除外する。Q00UJCPU にこの合計マップを適用しない。内部の `SD302/303`、高速型の `SD282-285`、合計の `SD308-311` は意味が異なる。[QCPU Appendix 3 p.551 詳細表](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=553)、[QnU pp.402-404](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=404)。

`SD647` は現在選択中のファイルレジスタファイルの容量を **1K 点（ZR は word デバイスなので 1K word）単位**で格納し、1K 点未満の端数を切り捨てる。拡張 D/W 使用時のファイル容量は ZR・拡張 D・拡張 W の合計であり、`SD647 × 1024` を ZR 単独の厳密な上限としない。ZR の厳密な点数判定には上記の適用条件を満たす `SD306-307` 等を使い、得られなければ `unknown` とする。[QnU pp.393,400,402-404](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=402)。

Q の拡張 D/W は内部領域の次へ**論理デバイス番号が連続**するが、物理領域は連続しない。経由モジュールの対応可否を別に確認し、未対応の経路で必要な場合はマニュアルに従い ZR 番号へ変換する。この変換と一括読出しの境界は経路別に検証する。QnU **p.405** の内部/拡張境界を跨がせない注意は、PLC の自動リフレッシュ・ブロックデータ命令等の指定に関する記述であり、**MC のすべての読出しを一律禁止する根拠にはしない**。[QnU p.402](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=404)、[QnU pp.405-406](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf#page=407)、[MC p.69 拡張 D/W 注意](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=71)、[SLMP p.39](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=40)。

## iQ-R / ENCPU: SD の実設定点数

以下は各先頭 SD が low word、次の SD が high word の **32-bit 点数**を表す。Z/LZ だけは単一 word。型式・ファームウェア・通信経路の対応が判明した標準 CPU にのみ用いる。[iQ-R CPU Appendix 5 pp.616-617](https://www.mitsubishielectric.com/dl/fa/document/manual/plc/sh082488eng/sh082488engf.pdf#page=618)。

| デバイス | 先頭 SD | デバイス | 先頭 SD | デバイス | 先頭 SD |
| --- | ---: | --- | ---: | --- | ---: |
| X | 260 | Y | 262 | D | 280 |
| M | 264 | B | 266 | W | 282 |
| SB | 268 | F | 270 | SW | 284 |
| V | 272 | L | 274 | Z（単 word） | 300 |
| S | 276 | T | 288 | LZ（単 word） | 302 |
| ST | 290 | C | 292 | ZR | 306 |
| LT | 294 | LST | 296 | RD | 308 |
| LC | 298 |  |  |  |  |

`S` の点数を表す `SD276-277` には SFC 対応条件がある。R00/R01/R02CPU は初期製品から、Rn(EN)CPU はファームウェア **12 以降**が対応する。適用機種・ファームウェアが確認できなければ `S` の点数判定は `unknown` とする。[iQ-R Appendix 5 p.616 の *2 注記](https://www.mitsubishielectric.com/dl/fa/document/manual/plc/sh082488eng/sh082488engf.pdf#page=618)、[Appendix 12 p.707 の SFC support](https://www.mitsubishielectric.com/dl/fa/document/manual/plc/sh082488eng/sh082488engf.pdf#page=709)。

表は設定点数に対応する SD を示す。T/ST/C/LT/LST/LC の点数は、接点・コイル・現在値の SLMP デバイスコードや bit/word/double-word 単位を示さない。存在しないデバイスコードへ一般化しない。[MC p.68 デバイスコード表](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70)、[SLMP pp.37-39](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=38)。

## 接続テストとタグ判定の設計案

1. PLC 設定待ちの保存は許す。保存時は型・設定書式・アドレス表記・長さの不正を検出するが、未接続というだけで型式別の点数を推測しない。「形式/型が不正」と「実機未確認」を別状態にする。
2. 本番利用前の**読出し専用**接続テストで 0101、機種別に適用可能な SD、対象タグの読出し可否を確認する。実機との通信は本設計文書では行っていない。実機への write 試験は別承認とする。Hub が I/O と結果表示、品質、監査を扱う。
3. WASM はタグの開始点と全占有点数を、オーバーフローを避けて半開区間 `[start, end)` で計算する。設定点数 `N` の `[0, N)` に**完全に収まるタグだけ**を読出し候補とする。U32 の 2 word、文字列の全 word、bit と word の変換単位を含め、途中で切ったタグを読まない。成立する候補をフレーム制限・経路の境界に従って分割する。部分的な旧値を新しい正常値として扱わない。
4. 判定は少なくとも `in_range` / `out_of_range` / `unknown` と取得世代を区別する。範囲外が確定したタグと、型式・SD・経路の情報不足による `unknown` を混ぜない。接続テストや通常読出しの失敗時に前回の正常値を「現在正常」として表示しない。型式と通信形式を識別できる場合の `unknown` は、利用者が実機設定を確認する責任で明示タグを 1 件ずつ通常読出し可能とする。読出し成功でも範囲保証に昇格しない。Hub への判定表示は構造化契約の後続課題とする。
5. 点数キャッシュは接続先、0101 の生応答と判別結果、取得世代、経路、接続セッションに結び付ける。接続先変更・設定変更・再接続・再取得で失効し、古い点数を新しい接続へ持ち越さない。
6. 将来の書込みは命令に含まれる**全範囲**を送信前に検査し、一部だけを送らない方針。実書込み、複数タグ/バッチの原子性と検査単位、WIT の拡張は後続設計と承認事項であり、現行実装の能力とはしない。

## 出典と未検証事項

- [QCPU User's Manual (Hardware Design, Maintenance and Inspection), Appendix 3 pp.549-553](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf) — SD282-311 と適用注記。上記の飽和・serial・Q00UJ 除外は表で確認した事実。
- [QnUCPU User's Manual (Function Explanation, Program Fundamentals), pp.400, 402-406](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080807eng/sh080807engae.pdf) — ファイルレジスタと拡張 D/W の配置・境界。ZR 変換の具体式は経路ごとに追加確認が必要。
- [MELSEC Communication Protocol Reference Manual, pp.68-69, 86, 166-168](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf) — デバイスコード/基数、コマンド、型名・コードと経由局。
- [MELSEC SLMP Reference Manual, pp.34-46, 138-140](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf) — フレーム、サブコマンド、デバイスアクセス、0101 応答。
- 本書のタグ状態、キャッシュ、接続テスト、write の事前検査は**設計案**。接続経路ごとの SD 読出し可否、ZR 変換、実機 read の結果、例外/品質の Hub 表示、WIT 変更、実書込みは**未検証・未実装**。
