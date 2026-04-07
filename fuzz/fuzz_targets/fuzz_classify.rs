#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // classify is a pure parser — should never panic on any input
    let _ = zenraw::classify(data);
});
