#![cfg(feature = "zencodec")]
use std::borrow::Cow;
use zencodec::decode::{Decode, DecodeJob, DecodePolicy, DecoderConfig};
use zenraw::RawDecoderConfig;
#[allow(dead_code)]
mod fixture {
    use zencodec::decode::{DecodeJob, DecoderConfig};
    use zencodec::inventory::{Inventory, Part};
    fn inventory(bytes: &[u8]) -> Inventory {
        zenraw::RawDecoderConfig::new()
            .job()
            .inventory(bytes)
            .unwrap()
    }
    include!("support/inventory_fixtures.rs");
    include!("support/dng_contract_fixture.rs");
    pub fn bytes() -> Vec<u8> {
        dng_fixture()
    }
}
#[test]
fn dng_policy_and_inventory_contracts() {
    let bytes = fixture::bytes();
    let dec = RawDecoderConfig::new();
    let expected = dec
        .clone()
        .job()
        .decoder(Cow::Borrowed(&bytes), &[])
        .unwrap()
        .decode()
        .unwrap()
        .metadata();
    for (policy, accepted) in [
        (DecodePolicy::none(), true),
        (DecodePolicy::strict(), false),
    ] {
        zencodec_testkit::check_decode_policy_admission(
            dec.clone(),
            &bytes,
            DecodePolicy::none(),
            policy,
            accepted,
            &expected,
        )
        .unwrap();
    }
    zencodec_testkit::check_inventory_dispatch(dec.clone(), &bytes).unwrap();
    #[cfg(feature = "inventory")]
    zencodec_testkit::check_inventory_input_limit(dec, &bytes).unwrap();
}
