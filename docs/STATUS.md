# STATUS

## Local Modbus WASM basic completion (2026-10-27)

Initial local plan: keep WIT `driver` unchanged, use its connection handshake for a configured FC3/FC4 probe, and use the existing optional `write-v1` world with a disabled default and full preflight. Verify numeric and Bool wire formats, boundary addresses, order, malformed responses, exception handling and input-only refusal with core, component and controlled Wasmtime fixtures.

- Dedicated D: worktree at main `efcab6b457bc3e4c8b89114fb5706939bd273192`. FC4-only connection verification uses an explicit `probe_address` via the existing connection handshake. Default remains `40001/u16` FC3. Static `address.probe()` is now `none` so a host does not repeat the fixed FC3 read after an FC4 handshake.
- FC1/FC2 Bool reads and disabled-by-default FC5 coil / FC16 holding-register writes are local guest/core work. FC2/FC4 writes reject before send. The old read-only world and optional write-v1 world both bind; the capability matrix is in [CONTRACT.md](CONTRACT.md#modbus-wasm-local-contract-update-2026-10-27).
- Hub must update its pinned guest, honor handshake steps, negotiate write-v1, and apply independent operational write policy and type checks before this can be used in production. No Hub repo or PLC was changed.
- Local verification: Rust 1.85.1 Modbus core 16 tests, wasm32 check and strict Clippy; Rust 1.98.1 component 14 unit plus one doc test and strict Clippy; real Wasmtime 14 controlled fixtures and strict Clippy; wasm32 component build and validation; both existing `driver` and optional `driver-with-write-v1` WIT target checks; scaffold guard, 17 CI classification tests and `git diff --check` passed. No hosted CI or physical device test was run.

## Local optional SLMP diagnostics-v1 stage (2026-10-26, unmerged)

- Built from merged main `94f65e4a534a864fdd5f56af68ef2dc7a562a8bd`. Added a separately versioned diagnostics interface and read-only/combined worlds without changing the existing `driver` or `driver-with-write-v1` definitions. The guest borrows the same connection resource and reports raw/display CPU identity, D/W and M/Y/L/B count evidence, exact/at-least/unknown scope, and one reasoned verdict per input tag. Invalid syntax and unsupported guest paths remain distinct from a range-unknown result.
- Diagnostic calls accept at most 256 tags and at most one extra same-connection SD request, Q SD290-294 or R SD262-275, when M/Y/L/B evidence is needed. D/W handshake evidence is reused. Other areas remain unknown in this v1 rather than receiving inferred ranges. A pending diagnostic excludes handshake/read/write; an abandoned or malformed response requires connection disposal. Hub owns I/O, connection generation, attempt token, presentation, quality and audit. Existing read/write allowlists and read optimization issue #871 are unchanged.
- Local Rust 1.98.1 checks passed: guest 18 unit and one doc test, 30 existing and four new real Wasmtime controlled fixtures, strict Clippy/fmt, wasm32 component build/validation and all four WIT world target checks, scaffold and 17 CI-classification cases, and `git diff --check`. A SHA-matched prior guest component binds to the old world and lacks the new diagnostic world; the new guest binds to all four. Host integration, hosted CI, and PLC communication remain unverified.

## Local typed-word and bit-write stage (2026-10-26, unpublished)

- Based on merged main `9c7d9ec87a467be779e94b0dd3670e566f2e917c`, an isolated worktree extends the existing disabled-by-default common write policy without changing WIT. D/W accept numeric `i16/u16/i32/u32/i64/u64/f32/f64` with full 1/2/4-word spans and both word orders. M/Y/L/B accept Bool bit points with Q/R 1401 bit-unit frames. Existing saved D/W ranges do not grant bit access; D3000/D3001 protection, preflight of the entire request, and no replay after an uncertain ACK remain.
- An enabled policy with a bit interval triggers same-connection Q SD290-294 or R SD262-275 after 0101 and the existing D/W count request. Bit writes require a confirmed standard Q/R model, their own configured point count, and one complete allowed interval. Refused or missing count remains Unknown and rejects the write. Q25PRH W/bit, X/input, special bit areas, timer/counter writes, word-bit RMW and bank switching are not enabled.
- The current Host still independently accepts only D/W u16 operational writes. Guest capability metadata does not authorize Host or production writes. This is local core/guest/controlled Wasmtime work only; no PLC/DB communication, saved production setting change, commit, push, PR or merge is included.
- Local verification: Rust 1.98.1 core and guest suites, 30 controlled Wasmtime cases, Clippy/fmt on all three SLMP crates, wasm32 component validation and both unchanged WIT-world target checks, scaffold guard, CI classification tests (17), and `git diff --check` passed. Core fixtures include independent Q/R bit-write bytes; runtime fixtures include confirmed M/Y/L/B bit counts, refused SD, mixed-width D and Q-internal-W writes, tag covers, and the prior read/write behavior. Hub integration and live PLC behavior remain untested.

## Previous local common-policy D/W u16 write stage (2026-10-26)

- Separate worktree from merged main `a706f33fc0dc4e867f74cfdfa2fdc1223a3ec62f`. The existing saved `d_u16_write_mode` and `d_u16_write_allowlist` keys remain stable; their schema labels describe one SLMP write control and one area-qualified range list. Existing `D3010-D3013` settings still permit only D. A W range such as `W1A-W1F` must be explicitly included in that same list. There is no per-device enable switch.
- D/W u16 writes share the 1401 word-run implementation. Standard R uses the same connection's SD280–283 counts; standard Q W uses its SD303 internal prefix. Complete ascending runs of 1–960 words, whole-item value checks, D3000/D3001 protection, ACK verification, and no automatic replay remain mandatory. Q25PRH W writes remain disabled; its SD303 read path does not change the write gate. Bit, word-bit, file/bank, timer/counter, and other numeric writes remain unsupported.
- This is guest/core local work only. The Host's independent `operational_write_item` and saved-policy parser still accept D/u16 alone, so this component **cannot** be used for operational W writes until the matching Host contract is implemented and tested. Rust 1.98.1 local checks passed 36 core fixtures, 15 guest unit tests plus one doc test, and 26 controlled real Wasmtime fixtures; the WIT world remained unchanged. No real connection, saved production setting change, publication, or merge is included.

## Local conditional Q/R read continuation (2026-10-26)

- Rebased as local work onto merged main `85eb10133649bf85fdeae1f76dd9ef586a643949`. Standard iQ-R `LTS/LTC/LSTS/LSTC` bit tags use a four-word `LTN/LSTN` read and extract coil/contact bits from the status word. Exact `Q25PRHCPU/004CH` now obtains SD302 and SD303 together; only internal W numeric tags wholly inside the SD303 guaranteed prefix are readable. General Q25 D stays range-Unknown. No extended W is inferred.
- Q ZR still lacks the required route/firmware/extension evidence. Block-switched R would need bank state and is not enabled. The merged common D/W u16 write policy remains intact; Q25PRH W writes remain rejected. No WIT, Hub, read optimizer, or PLC/DB communication changes are included in this read stage. Real PLC and Hub integration remain untested.

## Local remaining Q/R SLMP read stage (2026-10-26)

- Based on merged main `ee37212dad37fae7e8f0052c0cb8e6827229b1eb`, in a dedicated worktree. Added ordinary Q/R timer and counter contact/coil bit tags and one-word current tags, R long timer/retentive timer/long counter `i32` current tags and R-only `LCS/LCC` bit tags, R hexadecimal serial-number `ZR` numeric tags, and hexadecimal-suffix `Dn.h/bit` reads from one D word. R retentive names are `STS/STC/STN`; Q uses `SS/SC/SN`. Long timer current reads use four response words for one logical timer, with only the first two forming the current value. See [device table and restrictions](../drivers/slmp/README.md#remaining-qr-read-devices-local-controlled-stage).
- The handshake still obtains only D/W counts. New timer/counter/ZR ranges are unknown, so only explicit single-tag requests are made; successful reads do not establish a configured endpoint. Q25PRH general timer/file mapping, conditional Q ZR, block-switched R, and long timer contact/coil flag handling remain unsupported. D word-bit checks the existing D whole-word range. WIT, Hub, D/u16-only write gate, and deferred read optimization #871 remain unchanged. Local Rust 1.98.1 checks passed: 35 core fixtures, 13 guest unit tests plus one doc test, 25 real Wasmtime controlled fixtures, Clippy/fmt for the three SLMP crates, wasm32 component validation and both unchanged WIT world target checks, scaffold guard and 17 CI classification cases. These are controlled tests, not PLC or Hub integration. No PLC or DB communication was performed for this stage.

## Local standard Q internal W read stage (2026-10-26)

- On a recognized standard Q model, the guest reads SD302 and SD303 together on the same connection and retains separate internal D and W point counts. Hexadecimal W numeric tags are planned only when the complete tag fits within the SD303 guaranteed prefix. The old 32768 value is a lower bound, not an exact endpoint. Q25PRH keeps its SD302-only exception, while unknown W counts and Q extended W do not gain a read path. R W and existing Q D behavior remain separate.
- This stage changes no WIT, Hub, write gate, optimizer, or physical connection. Rust 1.98.1 local verification passed 34 SLMP core fixtures, 12 guest unit tests plus one doc test, and 24 controlled real Wasmtime fixtures. Q SD refusal leaves explicit D reads available at unknown range but W uncovered; Q25PRH retains SD302 alone. This is not a PLC or Hub integration test. [QCPU Appendix 3, printed pp.549-553](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=551).

## Q/R additional device reads (2026-10-26)

- Built on the unpublished M/X/Y/B bit-read worktree. L/F/V/SM decimal and SB hexadecimal bit tags, plus SD decimal and SW hexadecimal numeric tags, use their official Q/R 0401 device codes. These areas have no proven configured count in the current D/W-only handshake, so explicit reads remain individual and range-Unknown. Static write capability is false; the D/u16 write policy and both WIT worlds are unchanged.
- The new device codes and radices were compared with the [Mitsubishi MC code table, printed p.68](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080008/sh080008ab.pdf#page=70) and local `tyaro/slmp` rev `4500aea4e1cdf072e41eb844d6a4caf4d550feb8`. S, timer/counter variants, Q W/extended areas, file registers, and word-bit syntax remain separate work. No PLC or Hub integration was performed.
- Rust 1.98.1 local checks: SLMP core 34 fixtures, guest 11 unit tests and one doc test, real Wasmtime 23 controlled fixtures, Clippy for all three SLMP crates, wasm32 component build/validation and both unchanged WIT world target checks, scaffold guard, and 17 CI classification tests. This verifies framing and controlled decoding, not PLC model-specific support or a physical device range.

## SLMP M/X/Y/B bit-read stage (2026-10-26)

- This stage starts at merged main `c5ce7477b75f2d7dd263936b4fad21b3fa4e0a27`. It adds read-only `bit` tags for M decimal and X/Y/B hexadecimal device points; existing WIT, D/W numeric reads, and D/u16-only write gates remain unchanged.
- The current 0101/SD handshake obtains D/W counts, not bit-area counts. Explicit M/X/Y/B reads therefore remain range-Unknown and use one 0401 bit-unit request per tag, including Q25PRH. Local verification with Rust 1.98.1 passed: SLMP core 33 fixtures, guest 10 unit tests plus 1 doc test, real Wasmtime component 22 fixtures, Clippy for all three crates, core wasm32 check, unchanged WIT old and write-v1 world target checks, scaffold and CI classification guards (17 cases), and `git diff --check`. No Hub integration or PLC communication was performed.

## Local normal D/u16 write policy (verified simulation, 2026-10-26)

- Separate worktree from main `0e05d5b2a955923c8bc37237bd2b5ae4f0eb7253`. The guest declares `d_u16_write_mode` (default disabled) and `d_u16_write_allowlist` (default empty), matching the separately developed Host policy grammar. Enabled mode requires a nonempty, ascending, nonoverlapping list of up to 16 decimal D ranges, 256 ASCII characters, 1–960 words per range, within 24-bit addressing. Any configured or requested span crossing D3000/D3001 is refused.
- A normal request is one complete consecutive D/u16 run of 1–960 words within one allowlisted range. Same-connection SD280–281 proves a StandardR D span; exact Q25PRHCPU / 004CH and same-connection SD302 prove a Q internal D prefix. Unknown or insufficient evidence refuses the whole request before a write frame. The separate Q four-word test flag remains distinct. No WIT or Host policy changes, PLC/DB communication, or production enablement are included here.
- Rust 1.98.1 local checks: SLMP core 33 fixtures, guest 9 unit plus 1 doc test, real Wasmtime 20 controlled fixtures, fmt and Clippy for core/guest/runtime, wasm32 component validation and both existing WIT world targets, scaffold guard and 17 CI classification tests. This does not verify the separate Host integration or any physical write.

## Local Q25PRH four-word write simulation (2026-10-26)

- Based on merged PR #22 main `4d857074ba3bd81ec862d5c8729b4f862b738582`. The advanced test setting defaults to `disabled`; only explicit `enabled`, the exact `Q25PRHCPU / 004CH` 0101 pair, and a same-connection SD302 count guaranteeing all four internal D points gate one Q `1401/0000` frame for only `D3010..D3013/u16` with four validated values. Saving this setting does not open the production Host write gate. Refused, short, missing, or insufficient SD evidence fails closed; SD302=32768 is a guaranteed prefix. General Q read range remains unknown, and the StandardR write path is unchanged. No WIT, Hub, production permission, or PLC connection is changed. [QnPRH assignment, PDF p.22](https://www.mitsubishielectric.com/fa/document/technews/plc/fa-a-0214/faa0214a.pdf#page=22).
- The controlled Wasmtime fixture checks Q frame bytes, full-item ACK mapping, no-send rejection, and no replay after an uncertain ACK. This is local simulation, not a Q PLC write or confirmation of its physical port.

## Controlled simulation: contiguous StandardR D/u16 write (2026-10-26)

- This range-write stage starts at main `fd0db137139b25f2055ce5019cc1fea4bd0eafdc` (merged FC4/W PR #21). The FC4/W read behavior is retained; its reviewed head is unchanged.
- The optional write-v1 guest now validates an entire 1–960 item consecutive ascending D/u16 run against the exact StandardR SD280-281 D count, then produces one 1401/0002 frame. It preserves the single-item path and existing WIT. Gaps, duplicates, reversed or out-of-range addresses, unsupported types, and invalid values reject all items before sending. ACK/refusal maps to every cover; a pending write or uncertain ACK cannot be resent on that connection.
- The independent four-word D3010–D3013 frame fixture and controlled Wasmtime loopback passed, including simulated readback, whole-run rejection, device refusal, wrong-serial ACK, and pending retry rejection. Core fixture tests, 18 Wasmtime runtime tests, wasm32 component build/validation, both WIT world target checks, formatting, and core/guest/runtime Clippy passed. This is **not** an actual PLC write or Hub integration. The controlled Host fixture remains D3010/u16 single-item only, and production write is disabled. Real range write requires separate target/value/restoration approval.

## Stage 2: numeric reads in Modbus FC3 and SLMP D (2026-10-26)

- Based on main `0cc29009504651c2ec58012d64157cc0f2201102`, this change adds typed `i16/u16/i32/u32/i64/u64/f32/f64` read decoding to the two existing guests using the fixed shared codec rev. The existing WIT `int/uint/float` variants and downstream `f64` projection remain unchanged; 64-bit integer values beyond `2^53` can lose precision after that projection. No PLC or DB operation is part of this stage.
- SLMP remains D-only. Its read planner counts words while `covers` retains one original tag index per complete value; unknown ranges stay individual reads. The optional write extension remains StandardR D/u16 only. Controlled local checks passed: Modbus core 13 tests, native comparison 5, Modbus component 8 plus one doc test, and real Modbus Wasmtime 11; SLMP component 4 plus one doc test, real SLMP Wasmtime 16, and unchanged SLMP core regressions. Both component WIT targets and local scaffold/CI classification guards passed. Hub integration, PLC I/O, and DB I/O remain unverified.

## Previous stage: shared numeric codec for existing types (merged PR #19)

- [PR #19](https://github.com/tyaro/banto-driver-wasm/pull/19) merged as `0cc29009504651c2ec58012d64157cc0f2201102` from the reviewed head `cca93e13e81570fe998847ae3adfa6b42c849327`. No PLC or DB connection was used.
- Modbus FC3 U32 word joining and SLMP two-word SD point counts now call dependency-free `banto-register-codec::decode_u32` from Industrial Git rev `e4afe793cbae08ab4afd88467c8577010020957f`. Existing U16 byte decoding, public word-order settings, frame parsing, WIT, and write limits are unchanged. The six affected Cargo lockfiles pin the same rev.
- Local checks passed: Modbus core 10 tests at Rust 1.85.1, SLMP core 28 tests at Rust 1.98.1, both wasm32 core checks, native Modbus comparison 4 tests, both guest unit/doc tests and component WIT target checks, Modbus native/real Wasmtime comparison 8 tests, and SLMP real Wasmtime fixtures 14 tests. Formatting, Clippy, scaffold guard, and CI classification tests also passed. [Hosted CI 40249385032](https://github.com/tyaro/banto-driver-wasm/actions/runs/40249385032) passed all 9 jobs. Live PLC/DB behavior remains unverified.

## Current stage: Q25PRHCPU wire-only read

- The exact `Q25PRHCPU / 004CH` 0101 pair establishes Q word-read framing and plans SD302 only. A same-connection SD302 reply is retained separately for the gated local write test; ordinary explicit D reads remain individual with an unknown range verdict. Mismatched names/codes and 0101 refusals establish neither wire format nor SD302 mapping. [SLMP Reference Manual, printed p.139](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=141), [Mitsubishi QnPRH bulletin, PDF p.22](https://www.mitsubishielectric.com/fa/document/technews/plc/fa-a-0214/faa0214a.pdf#page=22).
- The existing WIT and standard Q/R behavior remain unchanged. No live PLC communication, redundant-route selection, write, or Hub implementation is part of this stage.
- Rust 1.98.1 local checks cover 26 SLMP core fixtures, the component unit/doc tests, 12 controlled real Wasmtime fixtures, formatting, Clippy, wasm32 target check, WIT target validation, scaffold guard, and 17 CI classification tests.

## Previous stage: verified contiguous D/u16 batch reads

- From merged PR #14 main, `read.plan` groups input-order adjacent D/u16 tags only when each address is within a verified configured range. Each step's ordered `covers` maps one response word to each original tag. The existing 0401 word-read limit of 960 points is enforced through the SLMP core; a longer run is split. Gaps, duplicates, unsupported specs and unknown-range tags break a run. Unknown-range explicit tags remain one request each and are not promoted by a successful read. [SLMP Reference Manual, printed p.46](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080956eng/sh080956engn.pdf#page=48).
- Unsupported syntax/type, verified R out-of-range addresses, and wire-number overflow remain uncovered for the Hub's item-level `Unsupported`. A whole-batch PLC end code becomes the same item-level `Device` error for each cover, without partial values; other planned steps can continue. Malformed or mismatched frames and plan/cover mismatches remain fatal under the host's close-and-reconnect contract.
- The WIT, Hub, native driver, real PLC I/O, and write path are unchanged. Structured discovery quality and unknown-model wire selection remain later work.
- Local Rust 1.98.1 verification: SLMP core 25 tests, component unit/doc tests, 11 real Wasmtime fixtures, component/runtime Clippy and formatting, WASM validation and WIT target check, scaffold guard, and 17 CI classification tests.

## Previous stage: SLMP guest with controlled Wasmtime fixtures (merged PR #13)

- Based on merged PR #12 main b3c108a886bd5d46de2c0066c0559a2489ca10f2, a separate SLMP component exports the existing WIT world without changing Modbus. One connection resource sequences 0101, the applicable basic D count SD (Q internal SD302 or R complete SD280-281), then a single D/u16 0401 read. Static D address parsing does not use connection state.
- Evidence and pending read plans are scoped to the connection resource. A newly created resource restarts 0101. With a recognized Q/R wire series, an explicit tag may be read one at a time when its range is unknown, at the operator's responsibility; success does not upgrade the range verdict. A Q address beyond internal D remains unknown, and an address within a saturated Q lower-bound prefix is verified. An R address beyond its exact complete D count is rejected. Unknown name/code pairs and 0101 refusals do not establish the Q/R 0401 wire format, so this guest plans neither SD nor tag reads for them.
- Five controlled Q/R, unknown-model, 0101-refusal, SD-refusal, and reconnection Wasmtime fixtures passed, along with SLMP core 25 tests and the component unit test. Hub display is covered by the design-only discovery contract; versioned WIT compatibility must be tested before implementation. Q SD308-311 total semantics remain open. PR #13 merged as `384565abc397d47307be97aa8fe9d45a141d6de7`; its final CI passed. No PLC, Hub, socket, native driver, or write operation was used; the existing WIT was unchanged.

## Previous stage: conditional Q extension fields (merged PR #12)

- After merged PR #11 (`506d89c97b37b81066eef8ad184c1e4fad1565fd`), this stage adds I/O-free SD282-289 plans and two-word decoders. QnU complete M/B count fields SD286-289 require independently confirmed route, compatible CPU version, and serial first five digits >= 10042. Fast internal-only D/W fields SD282-285 are limited to the five already-classified QnUDV names; QnUDPV is outside this classifier.
- SD286-289 already report total configured M/B points: do not add the old SD292/294 words. SD282-285 report internal D/W only. No synthesized D/W total, complete tag range, or SD647-derived capacity is asserted. Missing model/version/route/serial evidence or an incomplete pair stays Unknown. No PLC I/O, WIT, Hub, native, or write implementation is included.
- Rust 1.98.1 SLMP 25 tests, fmt, Clippy, and wasm32 type-check passed. PR #12 CI 40167139821 passed all jobs and the same-head review found no required code changes; merge SHA b3c108a886bd5d46de2c0066c0559a2489ca10f2.


## 今回の作業（QnU ZR の条件付き点数）

- [PR #10](https://github.com/tyaro/banto-driver-wasm/pull/10) は head `0e05f43` の CI・統合レビュー・競合なしを再確認して squash merge した。main の統合 SHA は `d30ac2f305fa13edaf7fc91daeaaefc9773f802c`。専用 worktree は未保存/固有ファイルと使用中プロセスがないことを確認して片付け、元 repo・他 worktree・branch は保持した。
- QCPU Appendix 3 p.550 の `SD306–307` 欄を原典の表画像で照合した。標準 QnU（Q00UJ 除外）で拡張 D または W を 1K 点以上設定した証拠、firmware と経路の適用確認がある場合のみ、ZR の low/high 点数を復号する。いずれか不明なら `Unknown`。SD647 の 1K 端数切捨て・共有容量から ZR 厳密上限を推定しない。
- ローカル Rust 1.98.1 の SLMP テストは既存17件と新規4件の計21件、整形・Clippy・wasm32 型検査、scaffold が成功。native にこの SD 判定はなく、実機 I/O・WIT/Hub・書込みは未実施。拡張 M/B、高速 D/W、D/W 総点数の適用条件は後続で個別に扱う。

## 今回の作業（SLMP 基本 SD 設定点数）

- [PR #9](https://github.com/tyaro/banto-driver-wasm/pull/9) は head `9dd08da` の CI・統合レビュー・指摘解消・競合なしを再確認して squash merge した。main の統合 SHA は `3cc8119e7b0117cb6d5f56b6d35e3a22c57dc75e`。専用 worktree は未保存/固有ファイルと使用中プロセスがないことを確認して片付け、元 repo・他 worktree・branch は保持した。
- `drivers/slmp` に標準 Q `SD290–304` と標準 iQ-R / ENCPU の基本 SD 点数の取得計画・復号を追加した。Q の D/W は内部点数のみと明示し、旧 M/B/内部 D/W の 32768 飽和は下限とする。R の pair と Z/LZ 単 word、S の firmware/SFC 条件を区別する。型式・経路・firmware または取得値が不明なら点数は `Unknown`。
- Q 拡張 M/B、高速 D/W、D/W 合計、条件付き SD306–307、SD647 はこの小段階の数値判定に使わない。公式マップと独立 fixture 5件を照合した。Rust 1.98.1 の SLMP 既存12件を含む計17件、整形・Clippy・wasm32 型検査、scaffold をローカル検証。実機通信、WIT/Hub、write は未実施。

## 今回の作業（SLMP 0101 型式名読取り）

- [PR #8](https://github.com/tyaro/banto-driver-wasm/pull/8) は head `8320935d` の CI・同版レビュー・競合なしを再確認して squash merge した。main の統合 SHA は `1f5932fd8e04820e6e9e6e585de7e625f4bad257`。専用 worktree は未保存・固有ファイルと使用中プロセスがないことを確認して片付けた。元 repo、他 worktree、branch は保持した。
- `0101/0000` の binary 4E 要求と固定 16 バイト ASCII 型名・2 バイト型式コードの復号を I/O 非依存コアへ追加した。型名の生データと表示名を保持し、標準 Q / iQ-R・ENCPU の公式な名前・コード組のみ分類する。未知組、RCPU/0360H、特殊 CPU は未対応型式として保持し、設定済みタグ範囲を推定しない。
- SLMP 公式マニュアル pp.138–141 を独立フィクスチャで確認。native には 0101 実装がないため、共通の 4E ヘッダー・応答検査のみ静的照合。GitHub Codex の同版レビューで見つかった Q10UDEHCPU / Q20UDEHCPU の別名漏れを公式表で確認して追加した。Rust 1.98.1 の SLMP テストは既存6件と新規6件が成功し、整形・Clippy・wasm32 型検査も成功。実 PLC 通信、WIT/Hub 結合、SD 範囲取得、書込みは未実施。

## 今回の作業（SLMP 0401 読取りフレーム）

- [PR #7](https://github.com/tyaro/banto-driver-wasm/pull/7) は head `66020f3a` の CI・統合レビューを再確認して squash merge した。main の統合 SHA は `5233d55a4d1c278c473e9c44df608c1423c1a629`。元 repo・他 worktree・branch は保持した。
- `drivers/slmp` に Q/R binary 4E・0401 の依存なし `no_std` コアを追加した。I/O はなく、M/X bit と D/W/SD word の読取りフレーム生成・応答解析のみ。SLMP Reference Manual の wire 形式と native `banto-driver-slmp/src/frame.rs` を別々に照合し、独立したバイト列フィクスチャで正常系・拒否・破損・点数と番号の境界を試験する。native が許す不正 bit nibble は仕様に従って拒否する。
- `slmp-guard` を Rust 1.98.1 の整形・Clippy・単体テスト・wasm32 型検査を行う `slmp-test` に置き換える。機種判別、SD 実設定範囲、タグ読取りテスト、WIT/Hub、実機通信、書込みは未実装。
- ローカルで Rust 1.98.1 の SLMP 整形・Clippy `-D warnings`・単体フィクスチャ 6/6・wasm32 型検査を通過。既存 Modbus コア 10/10、Node 変更判定 15/15、scaffold、CI YAML 構文、`git diff --check` も通過。実機・Hub 通信は行っていない。

更新日: 2026-10-25

## 今回の作業（FC3 U32 実 WASM）

- driver-wasm main `8498690b`（PR #6 の SLMP 文書を含む）から、未公開の SLMP 文書修整 `43b16d5` を引き継いだ。引用頁、R の見出しと SFC 条件、Q SD308–311 の根拠を明確化した。
- FC3・単一タグ U32 を既存 WIT の `uint(u64)` / `word-count=2` でコンポーネントに追加し、U16/U32 のアドレス、計画、復号、`word_order` の整合を検査する。native とコアで比較済みの U32 15件を実 Wasmtime 比較へ拡張し、U16 12件を維持する。
- [Hub PR #843](https://github.com/tyaro/banto-industrial/pull/843) は `4c6b126a` でマージ済み。Hub は固定ゲスト SHA で FC3・単一 U16 を扱う。この U32 版の Hub 登録・U32 境界検証は未実施である。
- manifest は能力の表示に合わせてプラグイン `version` を `0.2.0` にする。既存の任意 `word_order` を利用し、設定移行と WIT 変更がないため `settingsVersion=1`、`hubApi=1` を維持する。
- ローカル Rust 1.98.1 検証: コア10/10、固定 native 比較4/4、コンポーネント単体5/5・doc1/1、実 Wasmtime 比較8/8（U16 12件と U32 15件を含む）が成功。`wasm32-unknown-unknown` ビルドと wasm-tools 1.262.0 の component 生成・検証、3クレートの Clippy `-D warnings` が成功。Hub や実機を動かした結果ではない。

## 前回の作業（[PR #5](https://github.com/tyaro/banto-driver-wasm/pull/5)）

- main `be5753d003aa49d065405e30753a2a0903d3e24c` を確認し、FC3・単一タグ U32（2レジスタ）と word order を依存なしコアと固定版 native 比較に追加した。既存 U16 共有12件（10一致・余剰 PDU 2件の意図的拒否）は維持する。
- 固定版 native `a0d01aa2` の `frame_conn.rs` と `decode.rs` を照合した。`low_high` は先頭ワードが下位、`high_low` は先頭が上位。`create_connection` の未指定時は `low_high`、フォームの `settings_schema` 既定値は `high_low` である。コアの既定は前者に合わせ、フォーム側は変更しない。
- U32 の末尾開始アドレスは offset 65534（`465535`）まで有効。offset 65535（`465536`）は native のアドレス解析とコアの要求生成で拒否する。U32 の正常値・例外・ID 不一致・破損・余剰 PDU を native 比較で検証する。
- 後続 SLMP 工程には、機種・設定ごとのアクセス範囲を確かめる指定タグ読取りテスト命令の要件を引き継いだ。固定 probe と区別し、I/O は Hub エンジンに置く。今回の WIT・Hub・SLMP 実装は変更しない。

## 完了

- SLMP の標準 Q / iQ-R・ENCPU を対象とする [機種別点数・タグ判定の設計案](../drivers/slmp/README.md)を公式マニュアルと照合して追加した。Hub の表示・品質・監査は [Issue #845](https://github.com/tyaro/banto-industrial/issues/845) で追跡する。この追加は文書のみで、コード・WIT・native/Hub・実機通信は変更していない。

- README、作業ガイド、段階計画、軽量 CI の判定・ガードを用意した。
- 初回コミット `e6cbcf7e32f8299dda1afaf90c7579c01487c6cc` を main に公開した。
- Hub `a0d01aa2` の公開 API・WIT 案・native Modbus 実装を照合し、未決の境界を [CONTRACT.md](CONTRACT.md) に記録した。
- [PR #1](https://github.com/tyaro/banto-driver-wasm/pull/1) を `1e726f0c1bc523e9ef9ea8855699f7b00adb011e` で main にマージした。FC3・単一保持レジスタの依存なし `no_std` コアを追加し、余剰 PDU バイトを拒否する修正まで含む。
- このセッションの開始時に main が `97f5f86c80ca8d5391bab793c98d13b965e67faa` であると確認し、新規ブランチ `fc3-native-compare` を作った。Hub の main は照合元 `a0d01aa2beb007fc2636288c4e839410dece3fdc` と一致した。
- Hub の公開 `FrameDriver` を固定 Git 依存として実行する独立比較クレートを追加した。Hub の製品ソースはコピー・変更していない。WIT の未決事項と推薦案を [CONTRACT.md](CONTRACT.md) に更新した。
- [PR #2](https://github.com/tyaro/banto-driver-wasm/pull/2) を `aed41e6eb8db098da260ee874ca5afebc84d32c8` で main にマージした。比較試験・CI ゲート・契約照合文書を含む。比較元の head は `b758ae90dc3c4152df73b0dcb142f78df913015a`。
- 新規セッションで driver-wasm の main `77b9d8f7c0b8a62e190b3aa7f4f97462714686ae` を clean と確認した。Hub main `89de2953b59587839d93a182c0bc7c17179adb0e` を取得し、前回の `a0d01aa2` から API・Modbus・engine・WIT 設計に差分がないと確認した。未決境界の提案と決定後の比較経路を [CONTRACT.md](CONTRACT.md) に具体化した。製品コードと Hub 本体には変更を加えていない。
- 2026-10-25 に WIT 境界の5案について最終方針の承認を受けた。ローカル準備コミット `02b8fe1c63c1876ee7ba34ba457e0808dde4e5d1` を保持したまま再開し、driver-wasm main `77b9d8f7` と Hub main `aab51f437a303fd3ca4ef77ec73c46e54d43bcbc` を確認した。Hub の対象 API・Modbus・engine・WIT 設計は前回の `89de2953` から差分がない。承認された範囲を [CONTRACT.md](CONTRACT.md) に記録した。
- 承認済み境界に沿う [正本 WIT](../wit/driver.wit)、[FC3・単一 U16 ゲスト](../component/src/lib.rs)、[実 WASM 比較](../runtime-comparison/src/lib.rs) を追加した。`session` / `read` / `address` / `config` の全 export を持ち、ソケットと WASI は持たない。既存の native 比較フィクスチャを共用する。Hub 本体は変更していない。
- CI に `component-build` と `component-compare` を加え、WIT 検査、コンポーネント生成・検証、成果物を使った Wasmtime 実行を配線した。既存の Rust 1.85.1 コア CI と native 比較は維持した。`component/manifest.json` は初版メタデータの試案であり、Hub 側の検証器や配布形式ではない。
- 新規セッションで main `8a6d0a5d03e4e300731930a2c6ed685d28fbbc42`、本書・CONTRACT・両リポジトリの作業規約を確認した。Hub main は `aab51f437a303fd3ca4ef77ec73c46e54d43bcbc`。Hub の並行作業 [PR #842](https://github.com/tyaro/banto-industrial/pull/842) の変更ファイルを照合し、Hub 本体や別担当の作業ツリーは変更していない。独立ブランチ `wasm-host-prep` でテストホストの準備を進めた。
- 独立 Wasmtime ホストに試案 manifest の `settingsVersion` 必須正整数（`u32` に収まる値）・WIT `uint(u64)` の U16 型と範囲の拒否試験を追加した。ローカル TCP の制御した応答で `probe` → `parse` → `read.plan` → 送受信 → `read.decode` を通し、正常値・機器例外・破損・ID 不一致を確認する試験も追加した。これは Hub loader や `FrameSession` 経由の試験ではない。
- [PR #4](https://github.com/tyaro/banto-driver-wasm/pull/4) の head `2db459fb437c143b3dee9b442fd5fffe516e0c2a` を、`ed845af9bc1f062b510d804e03d319eb4f3b8fd3` で main に squash merge した。Hub 本体とその並行ブランチには変更を加えていない。

## 検証

- SLMP 文書変更で Node.js 24.22.1 の `node --test .github/scripts/ci-changes.test.mjs` が 15/15 成功。`node .github/scripts/check-scaffold.mjs` と `node .github/scripts/check-scaffold.mjs slmp` が成功。`git diff --check` が成功。Rust・実機通信は実行していない。
- 今回のローカル Rust 1.98.1 検証: コア単体10/10、native 比較4/4（U16 共有12件、U32 15件、末尾範囲・未指定既定値）、コアと比較クレートの整形・Clippy、コアの `wasm32-unknown-unknown` 型検査、既存コンポーネント単体4/4・doc1/1、既存 Wasmtime 比較クレートのテストコード型検査が成功。Node.js の変更判定15/15、構成ガード、`git diff --check` も成功した。U32 の実 WASM 比較を実施した結果ではない。
- PR #5 初回 head `993b5d2d211e99af397d9e477266939c75fa7cf9` の [CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40157741035) は終端まで成功。`modbus-test` / `native-compare` / `component-build` / `component-compare` / `ci-gate` を含む7ジョブが成功し、対象外の SDK・SLMP ガード2ジョブは skipped。
- Node.js 24.19.0 で `node --test .github/scripts/ci-changes.test.mjs` が 12/12 成功。
- `node .github/scripts/check-scaffold.mjs` と `sdk` / `slmp` 指定のガードが成功。
- `git diff --check` が成功。CI 定義は YAML として解析できた。
- [初回公開の GitHub Actions](https://github.com/tyaro/banto-driver-wasm/actions/runs/40109757859) は全ジョブ成功。
- [STATUS 更新後の GitHub Actions](https://github.com/tyaro/banto-driver-wasm/actions/runs/40109822669) も全ジョブ成功。
- Modbus コアは Rust 1.85.1 で単体テスト 7/7、整形、Clippy、`wasm32-unknown-unknown` への `cargo check` が成功。[PR #1 の最終 CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40111720556) も全ジョブ成功。WASM コンポーネントを実行した結果ではない。
- [PR #1 マージ後の main CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40112148506) は全ジョブ成功。
- `cargo +1.98.1 test --manifest-path comparison/Cargo.toml --locked` で 2/2 成功。FC3 単一レジスタの共通応答フィクスチャ 12 件中 10 件で値・エラー分類が一致。余剰 PDU の正常・例外 2 件は native が受理し、新コアが `Framing` で拒否する意図的な差として検証。要求 MBAP、native の `probe`、`settings_version=1`、TCP の通信間隔ゼロも検証した。
- 今回のローカル確認: Node.js 24.19.0 の変更判定テスト 13/13、構成ガード、Rust 1.85.1 の core テスト 7/7・Clippy・`wasm32-unknown-unknown` の型検査、Rust 1.98.1 の比較テスト 2/2・Clippy・整形、`git diff --check`、CI YAML 解析が成功。[PR #2 の CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40113142990) は `native-compare` / `modbus-test` / `ci-gate` を含む全 7 ジョブ成功。
- [PR #2 マージ後の main CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40113609047) も全 7 ジョブ成功。
- 今回の文書変更では Node.js 24.19.0 の `node --test .github/scripts/ci-changes.test.mjs` が 13/13 成功し、`node .github/scripts/check-scaffold.mjs` の通常・`sdk`・`slmp` 指定と `git diff --check` が成功した。
- このクラウド環境では公式 rustup から Rust 1.98.1 のみを導入し、`wasm32-unknown-unknown`、wasm-tools 1.262.0、wit-bindgen 0.63.0 を使った。Wasmtime 50.0.0 は比較クレートのライブラリとして使用し、CLI は導入していない。Rust 1.85.1 は新規導入していない。
- 新規 WIT は `wasm-tools component wit` と `validate` に成功。`cargo +1.98.1 build --manifest-path component/Cargo.toml --target wasm32-unknown-unknown --locked` でコアモジュールを作り、`wasm-tools component new`、`validate`、`component targets ... --world driver` に成功。これはコアの `wasm32` 型検査とは別の実コンポーネント生成である。
- `BANTO_COMPONENT_WASM=... cargo +1.98.1 test --manifest-path runtime-comparison/Cargo.toml --locked` は3/3成功。実コンポーネントを Wasmtime 50.0.0 でインスタンス化し、共有12フィクスチャの10一致・2厳密拒否、probe / parse / config / handshake / plan を検査した。別テストで Wasmtime の `memory.grow` 上限失敗が `-1` を返し trap ではないこと、`unreachable` trap と epoch 割込を型付き原因として区別できることを確認した。
- 最終ローカル確認: Rust 1.98.1 でコア単体7/7、コンポーネント単体4/4・doc1/1、native 比較2/2、実 WASM 比較3/3、各クレートの整形・Clippy、コア `wasm32` 型検査が成功。Node.js 24.19.0 の変更判定15/15、構成ガードの通常・`sdk`・`slmp`、CI YAML 解析、manifest の現値確認、`git diff --check` も成功した。
- [PR #3](https://github.com/tyaro/banto-driver-wasm/pull/3) を Ready で公開した。CI の成果物パスをリポジトリ基準の絶対パスに修正し、同じ配置で実 WASM 比較3/3を再実行した。[修正版 CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40150441856) は `component-build` / `component-compare` / `ci-gate` を含む全9ジョブ成功。Rust 1.85.1 のコア CI も成功した。修正前の実行は次の push によりキャンセルされた。
- PR #3 の最終 head `742aa507ef8304258ca4a61bcbf8e737f0214383` は [CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40150933235) 全9ジョブ成功、未解決レビューなしと確認し、`9e1a77010d62276736718a7c6f832c296e5aa6c6` で main に squash merge した。main を同 SHA へ fast-forward して引き継いだ。
- PR #3 後の [main CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40151386077) は `component-build` / `component-compare` / `ci-gate` を含む全9ジョブ成功と確認した。これはこのセッションのテストホスト追加前の main に対する結果である。
- 新規セッションの環境では公式 rustup から Rust 1.98.1 のみを導入し、`wasm32-unknown-unknown`、wasm-tools 1.262.0、wit-bindgen 0.63.0（コンポーネント依存）、Wasmtime 50.0.0（テスト依存）を確認した。Rust 1.85.1 は導入していない。正本 WIT の検証、実コンポーネントのビルド・検証・world 対応、コンポーネント単体4/4・doc1/1、独立 Wasmtime 比較6/6、runtime-comparison の整形・Clippy、Node 変更判定15/15・構成ガード・`git diff --check` が成功。6件には manifest・U16 拒否とローカル TCP 制御通信を含む。
- PR #4 の [対象 CI](https://github.com/tyaro/banto-driver-wasm/actions/runs/40152706179) は完了・成功。`modbus-test` / `native-compare` / `component-build` / `component-compare` / `ci-gate` を含む7ジョブ成功、変更対象外の SDK・SLMP ガード2ジョブは skipped。未解決レビューはなく、[統合レビュー](https://github.com/tyaro/banto-driver-wasm/pull/4#issuecomment-6392262134) も独立テストホスト準備の範囲で修正必須なしと確認した。レビューは Hub 製品境界の実装・実行を認定していない。

## 未検証

- SLMP の全機種・経路別 SD 取得、拡張 D/W の ZR 変換、全タグ種の範囲判定、実機 read と write、Hub 表示・品質・監査は未検証。既知 Q/R の D/u16 範囲 unknown は利用者責任で明示タグを 1 件ずつ読取り可と決定したが、型式不明・0101 拒否時は本ゲストで Q/R wire を確定できず読まない。WIT 拡張とバッチ書込み単位は未決。

- Hub の loader / runtime、manifest 検証、`FrameConnection` アダプター、U16 接続テストは PR #843 で実装された。この新しい U32 版ゲストの固定 SHA 更新、Hub 側 U32 型・範囲検査と読取り経路は未実施。
- 実機への Modbus TCP 通信、SLMP、FC3 以外、複数タグ、文字列、書き込みは未実装または未検証。U32 は独立 Wasmtime 比較の対象で、Hub 境界では未対応。ローカル TCP の制御通信は U16 の独立テストホスト経由のみで、ホストの受信処理は MBAP 長を直接読み、`plan.step.expect` の `PrefixSpec` は解釈していない。
- ローカルでは Rust 1.85.1 を導入・実行していない。CI では既存の 1.85.1 コア検証と WASM artifact のジョブ間再利用が成功した。実コンポーネントの試験は Wasmtime テストホスト経由であり、Hub runtime 経由ではない。

## 残件

- SLMP 実装時に公式表の機種条件、経路、アドレス基数、bit/word 単位、複合タグの全範囲をフィクスチャで検証する。本番前の read-only 接続テストと Hub [Issue #845](https://github.com/tyaro/banto-industrial/issues/845) の境界を合わせ、実機 write は別途承認を受ける。

- Hub 側 U32 登録と型・範囲検査は固定ゲスト SHA の更新を含む別作業で照合する。Hub の固定 U16 ゲストや adapter / CI は今回変更しない。
- SLMP の 0101 型式判別、実設定範囲取得、指定タグ読取りテストの段階分割を準備する。現行 WIT で範囲取得手順を表現できるか確認し、互換性に影響する変更は別途判断する。

## 次作業

1. この U32 版を独立 Wasmtime ホストで照合し、Hub U32 対応は別作業として境界を確認する。
2. Q / iQ-R SLMP の読み取りコア、0101 型式判別、実設定範囲取得、タグ読取りテストの段階を設計する。native 参照、公式仕様、独立フィクスチャを照合し、native の既存不具合は模倣しない。

次セッションの必要環境: Rust 1.98.1 のみ、`wasm32-unknown-unknown`、wasm-tools 1.262.0、wit-bindgen 0.63.0、テスト依存 Wasmtime 50.0.0、Node.js 24。新しいクラウド環境には前セッションの導入が継承されると仮定せず確認する。実通信の自動確認は制御したローカル TCP で行い、実機 PLC は使わない。MSRV 宣言と native descriptor の欠落時 `settings_version=1` は変更しない。

## 判断

- SLMP の実設定点数と機種別範囲判定は WASM 側の設計対象とし、Hub は I/O・接続テストの表示・品質・監査を担う。PLC 設定待ちの保存を許し、形式/型不正と実機未確認、範囲外と unknown を区別する。

- 開発場所を Hub と別の公開リポジトリ `tyaro/banto-driver-wasm` にする。
- 初回の製品目標は Modbus の WASM 読み取りと native 結果一致。既存ドライバの一括移植はしない。
- CI は Linux 中心。Modbus 領域は既存の Rust 1.85.1 コア検証、Rust 1.98.1 の native 比較、WIT 検査・実コンポーネントのビルド・Wasmtime 比較を対象とする。nextest 分割は計測後に判断する。
- native 比較を Modbus 領域の独立 CI ジョブとし、Hub `a0d01aa2` に固定する。余剰 PDU の受理差は新コアの厳格化を維持して明記する。WIT と Hub runtime の契約はこのリポジトリだけで確定しない。
- 初版の WIT 境界と適用範囲は [CONTRACT.md](CONTRACT.md) の「初版境界の決定」に従う。WASM manifest の `settingsVersion` は必須正整数、TCP gap はゼロ、FC3・単一 U16/U32 の生値は `uint(u64)`、host runtime 失敗は内部で型付き分類とする。Hub の固定ゲストは現時点で U16 のみ。

## 2026-10-25 SLMP optional write prototype (in progress)

- Branch: feat/slmp-r-d-write, based on origin/main f9b87d829b46587a63e01696097d6c3e8084f2d4. Existing read-only WIT world remains unchanged; wit/write-v1.wit adds an optional versioned world sharing session.connection.
- Local controlled fixtures cover old/new Wasmtime world negotiation, one StandardR D/u16 1401/0002 request on the same discovered session, preflight rejection, ACK/refusal/framing, and the existing read suite. No PLC communication, production host enablement, keys, or merge.
- Controlled host contract checks share the same Store, instance, and connection resource. PR and CI results are tracked separately. After-send transport failure must remain an indeterminate write result without automatic retry.
