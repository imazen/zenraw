# zenraw development tasks

# Run all tests with all features
test:
    cargo test --features rawler,darktable,exif,xmp,apple

# Run regression tests (requires darktable-cli + test files)
regress:
    cargo test --features rawler,darktable regression -- --nocapture

# Run unit tests only (fast)
unit:
    cargo test --lib

# Check all feature combinations
check:
    cargo check
    cargo check --features rawler
    cargo check --features darktable
    cargo check --features rawler,darktable
    cargo check --features zencodec
    cargo check --features exif
    cargo check --features xmp
    cargo check --features apple
    cargo check --features "rawler,darktable,exif,xmp,apple"
    cargo check --no-default-features --features std

# Clippy all features
lint:
    cargo clippy --features rawler,darktable,exif,xmp,apple

# Format
fmt:
    cargo fmt

# Run benchmarks (requires raw samples in /mnt/v/input/raw-samples/)
bench:
    cargo bench --features rawler

# Local CI sanity check
ci: fmt lint test

# Fetch RAW test samples from raw.pixls.us (one per format, ~125 MB)
fetch-samples:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="/mnt/v/input/raw-samples"
    mkdir -p "$dir"
    declare -A samples=(
        ["nikon_d40.nef"]="https://raw.pixls.us/data/Nikon/D40/DSC_1842.NEF"
        ["olympus_c5050z.orf"]="https://raw.pixls.us/data/Olympus/C5050Z/RAW_OLYMPUS_C5050Z.ORF"
        ["canon_eosr_craw.cr3"]="https://raw.pixls.us/data/Canon/EOS%20R/Canon_EOS_R_CRAW_ISO_100_crop_nodual.CR3"
        ["pentax_k5.pef"]="https://raw.pixls.us/data/Pentax/K-5/IMGP8063.PEF"
        ["canon_350d.cr2"]="https://raw.pixls.us/data/Canon/EOS%20350D/IMG_1707.CR2"
        ["panasonic_gf1.rw2"]="https://raw.pixls.us/data/Panasonic/DMC-GF1/panasonic_16-9.RW2"
        ["sony_nex3.arw"]="https://raw.pixls.us/data/Sony/NEX-3/RAW_SONY_NEX3.ARW"
        ["fuji_xt1.raf"]="https://raw.pixls.us/data/Fujifilm/X-T1/20171229_110916.RAF"
        ["iphone12pro.dng"]="https://raw.pixls.us/data/Apple/iPhone%2012%20Pro/IMG_1361.DNG"
    )
    for name in "${!samples[@]}"; do
        dest="$dir/$name"
        if [ -f "$dest" ]; then
            echo "Already have: $name"
        else
            echo "Downloading: $name..."
            curl -fSL -o "$dest.tmp" "${samples[$name]}" && mv "$dest.tmp" "$dest"
            echo "  -> $(du -h "$dest" | cut -f1)"
        fi
    done
    echo "Done. $(ls "$dir" | wc -l) files in $dir"
