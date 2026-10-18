#![cfg(feature = "zencodec")]

use zencodec::decode::{DecodeJob, DecoderConfig, DynDecoderConfig};
use zencodec::{CodecErrorExt, UnsupportedOperation};
use zenraw::RawDecoderConfig;

#[test]
fn capability_matches_compiled_inventory_and_both_dispatch_paths() {
    let config = RawDecoderConfig::new();
    assert_eq!(
        <RawDecoderConfig as DecoderConfig>::capabilities().inventory(),
        cfg!(feature = "inventory")
    );
    let generic = config.clone().job().inventory(b"malformed");
    let dynamic: &dyn DynDecoderConfig = &config;
    let erased = dynamic.dyn_job().inventory(b"malformed");
    if cfg!(feature = "inventory") {
        let inventory = generic.unwrap();
        inventory.validate().unwrap();
        assert_eq!(inventory, erased.unwrap());
    } else {
        assert_eq!(
            generic.unwrap_err().unsupported_operation(),
            Some(&UnsupportedOperation::Inventory)
        );
        assert_eq!(
            erased.unwrap_err().unsupported_operation(),
            Some(&UnsupportedOperation::Inventory)
        );
    }
}

// These equality constraints fail to compile if the inventory addition changes
// the published associated error type of either config or job.
#[test]
fn published_native_error_types_are_preserved() {
    fn config<C: DecoderConfig<Error = whereat::At<zenraw::RawError>>>(_: C) {}
    fn job<'a, J: DecodeJob<'a, Error = whereat::At<zenraw::RawError>>>(_: J) {}
    config(RawDecoderConfig::new());
    job(RawDecoderConfig::new().job());
}
