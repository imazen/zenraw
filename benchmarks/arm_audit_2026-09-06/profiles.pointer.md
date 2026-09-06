# Native RAW sampling profiles

Production `060b6a67`, Apple M4 Pro, Rust 1.98. Commands: `just arm-sample-decode <NEF> <output> rawloader` and the same with `rawler`. Each completes 200 Develop decodes; `sample` records five seconds at one-millisecond intervals, including startup. No memory-use claim is made from these samples.

- `/Users/lilith/work/codec-artifacts/zenraw-arm-audit/rawloader-profile.sample.txt` SHA256 `2d6e3da53cd7de64ee38d39b67072eca2335d0c80a754c4e3ad8741a1dbbd91f`

- `/Users/lilith/work/codec-artifacts/zenraw-arm-audit/rawloader-profile.run.log` SHA256 `ea2a23c621d32980bce092b12df7cd6ffc865cc75868fb372b728b527f5e29dc`

- `/Users/lilith/work/codec-artifacts/zenraw-arm-audit/rawloader-profile-command.log` SHA256 `d6578e731c444d0cef0b4f30e8c98034133152c1909301ba08fc89482d8994ef`

- `/Users/lilith/work/codec-artifacts/zenraw-arm-audit/rawler-profile.sample.txt` SHA256 `6d7ba3acdb302d15be899843eb82523665a22e24f466b3e3dfa13eea150091ca`

- `/Users/lilith/work/codec-artifacts/zenraw-arm-audit/rawler-profile.run.log` SHA256 `2096152a89965d53738c0c78cbec2344e34d59c2be313f3615008e49a920896c`

- `/Users/lilith/work/codec-artifacts/zenraw-arm-audit/rawler-profile-command.log` SHA256 `bbedcda6904e9607cea4f4f6763bd0d410f63efee453e33b5277cb94e4173707`


Top-of-stack counts: rawloader `powf` 1114, NEF `do_decode` 779, Malvar demosaic 578; rawler `powf` 1436, `apply_dt_sigmoid` 657, NEF `do_decode` 467, Malvar interior 272. These are sampled leaf counts, not independent wall-time measurements. Exact scalar transcendental math is a substantial cost on this fixture; replacing it with approximate SIMD math would need pixel-equivalence evidence that this audit does not have.
