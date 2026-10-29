# Optional Pulse adapter qualification

Source `51411a7bc5ad5581ecd22360e83b0454f52809fd`, with core `feb09f9f`
and pixels `283655c5`, tested in an export outside workspace path overrides.
The matching dependency lock is retained beside this report (copy to Cargo.lock
for reproduction). The repository does not track a root lock, so this is a
resolved graph receipt, not proof of historical transitive dependency identity.

```sh
cargo metadata --locked --format-version 1
just pulse-adapter-check
just incremental-admission-check
```

Passed: owned policy/child sharing (1), native progress/cancellation (9 rawloader,
9 rawler), incremental admission (8 rawloader, 8 rawler), and default-backend
all-target strict Clippy. The admission controls include exact strided samples,
input/memory caps, early headers, terminal failures and retained cancellation.
Per-call None preserves retained job policies; Some(StopOnly) still cancels
before probe/sink effects. The original assertions are retained.

The incremental method now accepts Option<&dyn Pulse> and forwards the option
unchanged to WholeFileIncremental. Names use InputRequest. Full observations
survive native preflight and cached headers; owned stop combinations keep child
sharing. No native pixel algorithm changed.

Initial failures are retained: sharing-test E0505 (drop empty child list before
owner), erased-cause test E0599 (inspect the typed cause), and strict Clippy's
obsolete Unstoppable import. Legacy explicit-stop tests keep scoped deprecation
allowances to retain that compatibility coverage; production warnings are not
suppressed.

Final receipt on r7900x: `run-heavy: done rc=0 11s | peak-RSS 0.27GiB | min-avail 14774MiB | peak-load 1.49`.
This is qualification resource usage, not an encoding/decoding performance result.
Standalone fuzz graph, full inventory/corpus, API snapshots, WASM and platform
qualification remain outstanding. No crate publication claim.
