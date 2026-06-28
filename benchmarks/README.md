# zenraw benchmarks

zenraw ships **profiling harnesses, not a cross-codec comparison**. There is no
single "fastest RAW decoder" claim to make here — coverage and color accuracy
differ per backend and per camera — so the harnesses measure zenraw's own decode
pipeline (throughput and heap behaviour) so regressions are visible. No
performance numbers are committed: they are hardware- and corpus-dependent, and
the RAW corpus is not redistributable. Reproduce locally with the steps below.

## Corpus

RAW files are large and licensing-encumbered, so none are committed. Fetch one
public sample per format (~125 MB total, from [raw.pixls.us](https://raw.pixls.us))
into the canonical local path `/mnt/v/input/raw-samples/`:

```sh
just fetch-samples
```

The harnesses skip any sample that is missing, so a partial corpus still runs.

## Throughput (`benches/decode_bench.rs`)

Built on [zenbench](https://github.com/imazen/zenbench) (criterion-compat). Times
the full `decode` and `probe` paths across the sample set, plus a demosaic-method
A/B (`Bilinear` vs `MalvarHeCutler`) on one NEF.

```sh
git clone https://github.com/imazen/zenraw && cd zenraw
git checkout <commit-sha-you-are-measuring>
just fetch-samples              # populate /mnt/v/input/raw-samples/
just bench                      # == cargo bench --features rawler
```

Methodology notes:

- **IO excluded from the timed region** — each sample's bytes are read into a
  `Vec<u8>` before the measured loop; the closure decodes from `&[u8]`.
- **Single-threaded** — the decode pipeline is serial; there is no rayon fan-out
  to pin. Build **without** `-C target-cpu=native` (runtime SIMD dispatch via
  archmage is what ships) so numbers reflect what users get.
- **Throughput is reported in input bytes/s** (`Throughput::Bytes`), because the
  RAW input size, not the decoded pixel count, is the natural unit for a decoder.
- Switch the backend with the feature flag: `cargo bench --features rawler`
  exercises the rawler path; default features exercise rawloader.

## Heap allocations (`examples/heaptrack_decode.rs`)

Decodes a RAW/DNG file from bytes in a loop (default 8×) so per-decode allocation
churn separates cleanly from one-time setup. Needs `heaptrack` installed.

```sh
just heaptrack-decode                                   # nikon_d40.nef, 8 iters
just heaptrack-decode /path/to/file.dng 16              # custom file + iters
heaptrack_print /tmp/zenraw-ht.zst | less              # inspect the trace
```

A heaptrack run recorded under `[Unreleased]` in
[CHANGELOG.md](../CHANGELOG.md) found the develop pipeline allocation-efficient:
the raw / RGB-f32 / output buffers are reused, so each *additional* decode adds
only a handful of allocations, and the rawloader backend's bundled camera-metadata
database is a one-time process-global cache rather than a per-decode leak. Re-run
the command above to measure peak heap on your own hardware and corpus.

## Recording results

If you capture numbers worth keeping, commit them per the repo conventions as
`benchmarks/<topic>_<YYYY-MM-DD>.{md,csv,tsv}` with a header recording the git
commit, hostname, CPU, RAM, OS, `rustc -V`, feature set, and the exact command —
enough to reproduce the row from the file alone.
