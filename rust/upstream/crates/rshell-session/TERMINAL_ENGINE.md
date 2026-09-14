# Terminal-engine decision record

Decision: **GO**

The sole Alacritty adapter passed the unchanged terminal-engine measurement contract against the
frozen implementation identity below. Normal verification fails closed if any runtime field,
threshold, fixture value, or recorded measurement differs.

## Measurement contract

- Command: `cargo bench -p rshell-session --bench terminal_engine --locked`
- Candidate command: `cargo bench -p rshell-session --bench terminal_engine --locked -- --record-candidate`
- Selected sole adapter: `alacritty-terminal@0.26.0`
- Measured implementation commit: d37e44ff2c20474bcfcf8670332464b6b25b5d7d
- Platform and toolchain: Windows x86_64-pc-windows-msvc; rustc 1.95.0 (59807616e 2026-04-14)
- Throughput sample 1: 55.158777 MiB/s
- Throughput sample 2: 47.245962 MiB/s
- Throughput sample 3: 39.302994 MiB/s
- Throughput sample 4: 42.060847 MiB/s
- Throughput sample 5: 46.943365 MiB/s
- Throughput median: 46.943365 MiB/s
- 120x40 frame p95: 0.550100 ms
- Scrollback digest: `sha256: bf34a24ce64fe4568b31353096d538dc0bb083c52ba0dab3099595b62d1dfcd5`

The executable processes exactly 104857600 bytes in each of five fresh-engine samples and
requires a median of at least 40.0 MiB/s. It measures 1000 full-dirty 120x40 frame observations
and requires nearest-rank p95 below 16.0 ms. Its scrollback oracle feeds exactly 1000 generated
labels with explicit trailing CRLF, verifies rendered rows before hashing, and canonicalizes the
verified labels with LF and no final LF.

Candidate mode completes every section even when a performance threshold misses. After exact
row equality, it emits the complete version 1 field set with the verified digest and
`decision=NO-GO`, writes a static threshold diagnostic to stderr, and exits nonzero. This output
is diagnostic only: it must not populate this record or the fixture, and the normal gate rejects
it. A passing candidate continues to emit `decision=CANDIDATE`.

## Recording instructions

1. Freeze the implementation in an orchestrator-authorized commit and require a clean tracked
   working tree.
2. Record `git rev-parse HEAD` as the measured implementation commit without changing that
   commit during measurement.
3. Run the candidate command. It must satisfy the unchanged thresholds, exact rendered-row
   equality, and emit every version 1 field with `decision=CANDIDATE`.
4. Copy the emitted lowercase SHA-256 into `tests/fixtures/vt/canary.json`.
5. Replace every unrecorded value above with the candidate command's source identity, platform,
   toolchain, five samples, median, p95, and digest. Change the decision only after recording
   exact evidence.
6. Run `pwsh -NoProfile -File scripts/qa/terminal-engine-gate.ps1`. The normal verifier accepts
   a GO record only when the fixture, executable output, thresholds, source record, and decision
   agree exactly.
