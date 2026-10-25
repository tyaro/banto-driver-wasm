# SLMP discovery contract proposal

Status: design only. This PR does not change WIT, the Hub adapter, or the registered guest.

## Current guest and evidence

The existing banto:driver@1.0.0 world provides session.connection.handshake-next and read.plan/read.decode, but no structured way to report CPU identity, configured counts, or why a count is unknown. The small SLMP guest proves one path with controlled responses: 0101/0000 identifies a standard Q/R name/code pair, a 0401 SD read obtains the configured D count, and a later 0401 D/u16 read uses that count. Each connection.create constructs fresh evidence. The guest does not store evidence globally or across reconnections.

0101 success alone is not a range proof. For the tested path, the CPU name/code pair, route echo, SD response, and read plan belong to the same connection resource. [QCPU Appendix 3](https://dl.mitsubishielectric.com/dl/fa/document/manual/plc/sh080483eng/sh080483engbc.pdf#page=552) describes Q SD302 as internal D and its legacy saturation at 32768; [iQ-R Appendix 5](https://www.mitsubishielectric.com/dl/fa/document/manual/plc/sh082488eng/sh082488engf.pdf#page=618) gives R SD280-281 as a low/high pair. A Q D total including an extended area remains unresolved in this stage. No real PLC was contacted.

## Proposed Hub-facing snapshot

A future structured snapshot should contain:

| Field | Meaning |
| --- | --- |
| connection-generation | Host-bound identifier for the one endpoint, driver settings, and connection resource that produced the evidence. |
| model | Raw 16-byte 0101 name, display name, type code, and classification; keep unknown names/codes intact. |
| counts | Device area, scope (total, internal, or another explicitly defined scope), unit (bit or word points), state (exact, at-least, unknown), and optional numeric count. |
| reason | Stable reason for unknown, such as unsupported name/code, route/firmware/serial not confirmed, conditional SD not applicable, SD read failure, or unresolved SD semantics. |

Hub should invalidate the snapshot on endpoint, settings, driver identity, or connection-generation change and on reconnect. Hub owns acquisition time, presentation, quality, and audit records. A prior successful value must not be displayed as a current normal value after invalidation. The snapshot must not contain credentials.

## Compatibility options to verify before any WIT edit

1. Recommended: a separately versioned discovery interface/world, with an explicit Hub capability check and an old-world path for existing Modbus guests. Verify with Wasmtime 50 that the old Hub binding still instantiates the old component and that a new binding can read the structured snapshot from the SLMP guest. A new world or additional export must not be assumed compatible without this fixture.
2. Reuse an existing string or error channel for display data. This avoids a new binding but loses typed counts, scope, generation, and stable unknown reasons, and can make a successful read look like verified range evidence. This is not recommended.

The read policy is decided for this narrow guest: after a recognized Q/R name and code establish a supported wire format, an explicit, syntactically valid D/u16 tag may be read one at a time when its configured range is unknown. The user must check the PLC configuration before relying on it. SD refusal, Q addresses beyond the verified internal D count, and addresses beyond an `AtLeast` prefix remain unknown; a successful ordinary read does not promote them to verified in-range. An R address at or beyond its verified complete D count is rejected. A tag wholly inside an `AtLeast` prefix is verified in-range. An unknown name/code pair or 0101 refusal does not establish Q/R 0401 framing and therefore does not permit a tag read in this guest; this is a wire-series limitation, not a verified range-outside verdict. Unsupported syntax, type, and route semantics remain errors. Hub quality and display of this distinction require the future structured contract above. 0101 identifies the model; it cannot check every device address. A later registered-tag, individual read check is only a candidate and would still not prove the configured range. No full-address scan or write path is proposed here.
