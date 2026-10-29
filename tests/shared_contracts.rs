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

#[cfg(test)]
mod incremental_admission {
    use super::*;
    use enough::{Stop, StopReason};
    use zencodec::ResourceLimits;
    use zencodec::decode::{
        DecodeRowSink, DynIncrementalDecoder, Event, Input, InputRequest, SinkError, StepError,
    };
    use zenpixels::{PixelDescriptor, PixelSliceMut};
    use zenraw::{RawError, RawLimitKind};

    // Padded caller-owned rows exercise the shared helper's real sink path.
    #[derive(Default)]
    struct Sink {
        storage: Vec<u32>,
        stride: usize,
        width: u32,
        height: u32,
        begins: usize,
        finishes: usize,
    }

    impl DecodeRowSink for Sink {
        fn begin(
            &mut self,
            width: u32,
            height: u32,
            desc: PixelDescriptor,
        ) -> Result<(), SinkError> {
            self.begins += 1;
            self.width = width;
            self.height = height;
            // zenpixels0.2.16 requires whole-pixel stride. Four pixels also
            // align the u32-backed storage for either RGB16 or RGBF32.
            let unit = 4 * desc.bytes_per_pixel();
            self.stride = (width as usize).div_ceil(4) * unit + unit;
            self.storage = vec![0xA5A5_A5A5; self.stride * height as usize / 4];
            Ok(())
        }

        fn provide_next_buffer(
            &mut self,
            y: u32,
            height: u32,
            width: u32,
            desc: PixelDescriptor,
        ) -> Result<PixelSliceMut<'_>, SinkError> {
            let bytes = bytemuck::cast_slice_mut(&mut self.storage);
            PixelSliceMut::new(
                &mut bytes[y as usize * self.stride..],
                width,
                height,
                self.stride,
                desc,
            )
            .map_err(|e| e.to_string().into())
        }

        fn finish(&mut self) -> Result<(), SinkError> {
            self.finishes += 1;
            Ok(())
        }
    }

    fn decoder(limits: ResourceLimits) -> Box<DynIncrementalDecoder> {
        RawDecoderConfig::new()
            .job()
            .with_limits(limits)
            .dyn_incremental_decoder(&[])
            .unwrap()
    }

    fn assert_denial(dec: &mut DynIncrementalDecoder, input: Input<'_>, kind: RawLimitKind) {
        let mut sink = Sink::default();
        let error = dec.step(input, &mut sink, None).unwrap_err();
        let StepError::Codec(error) = error else {
            panic!("unexpected error: {error:?}")
        };
        let native = error
            .get_ref()
            .downcast_ref::<whereat::At<RawError>>()
            .unwrap();
        assert!(
            matches!(native.error(), RawError::LimitExceeded(found, _) if *found == kind),
            "{native}"
        );
        assert!(dec.info().is_none(), "denial must precede HeaderReady");
        assert_eq!(sink.begins, 0);
        assert_eq!(sink.finishes, 0);
        assert!(dec.requested().is_empty());
        assert!(matches!(
            dec.step(Input::empty(), &mut sink, None),
            Err(StepError::Finished)
        ));
    }

    #[test]
    fn announced_limits_reject_before_need_at_every_offset() {
        for offset in [0, 17] {
            let mut dec = decoder(ResourceLimits::none().with_max_input_bytes(10));
            assert_denial(
                &mut *dec,
                Input::new(offset, &[]).with_total_len(11),
                RawLimitKind::InputSize,
            );
            let mut dec = decoder(ResourceLimits::none().with_max_memory(10));
            assert_denial(
                &mut *dec,
                Input::new(offset, &[]).with_total_len(11),
                RawLimitKind::Memory,
            );
        }
        let mut dec = decoder(ResourceLimits::none().with_max_input_bytes(0));
        assert_denial(
            &mut *dec,
            Input::empty().with_total_len(1),
            RawLimitKind::InputSize,
        );
    }

    #[test]
    fn unknown_growth_is_bounded_and_zero_input_cap_has_only_eof_lookahead() {
        for cap in [0usize, 8] {
            let mut dec = decoder(ResourceLimits::none().with_max_input_bytes(cap as u64));
            let bytes = vec![0u8; cap + 1];
            let mut sink = Sink::default();
            let p = dec
                .step(Input::new(0, &bytes[..cap]), &mut sink, None)
                .unwrap();
            assert!(matches!(p.event, Event::Need));
            assert_eq!(dec.requested(), &[InputRequest::at_least(0, cap + 1)]);
            assert_denial(&mut *dec, Input::new(0, &bytes), RawLimitKind::InputSize);
            assert_eq!(sink.begins, 0);
        }
        // A hard memory ceiling never gains the input cap's lookahead byte.
        let mut dec = decoder(ResourceLimits::none().with_max_memory(8));
        let mut sink = Sink::default();
        let p = dec.step(Input::empty(), &mut sink, None).unwrap();
        assert!(matches!(p.event, Event::Need));
        assert_eq!(dec.requested(), &[InputRequest::at_least(0, 8)]);
        assert_denial(&mut *dec, Input::new(0, &[0; 8]), RawLimitKind::Memory);
        let mut dec = decoder(ResourceLimits::none().with_max_memory(0));
        assert_denial(&mut *dec, Input::empty(), RawLimitKind::Memory);
    }

    #[test]
    fn address_space_and_window_overflow_are_terminal() {
        for input in [
            Input::empty().with_total_len(u64::MAX),
            Input::new(u64::MAX, &[0]),
        ] {
            let mut dec = decoder(ResourceLimits::none());
            let mut sink = Sink::default();
            let error = dec.step(input, &mut sink, None).unwrap_err();
            let StepError::Codec(error) = error else {
                panic!("unexpected error: {error:?}")
            };
            let native = error
                .get_ref()
                .downcast_ref::<whereat::At<RawError>>()
                .unwrap();
            assert!(matches!(native.error(), RawError::OutOfMemory(_)));
            assert!(dec.info().is_none());
            assert_eq!(sink.begins, 0);
            assert!(matches!(
                dec.step(Input::empty(), &mut sink, None),
                Err(StepError::Finished)
            ));
        }
    }

    #[test]
    fn early_header_waits_for_exact_cap_eof_and_matches_one_shot() {
        let bytes = fixture::bytes();
        let expected = RawDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(&bytes), &[])
            .unwrap()
            .decode()
            .unwrap();
        let desc = expected.descriptor();
        let expected_bytes = expected.into_buffer().copy_to_contiguous_bytes();
        let mut dec = decoder(ResourceLimits::none().with_max_input_bytes(bytes.len() as u64));
        let mut sink = Sink::default();
        let prefix = Input::new(0, &bytes[..bytes.len() - 1]);
        let p = dec.step(prefix, &mut sink, None).unwrap();
        assert!(matches!(p.event, Event::Metadata(_)));
        assert!(dec.info().is_some());
        assert_eq!(
            sink.begins, 0,
            "header availability is not pixel availability"
        );
        assert!(matches!(
            dec.step(prefix, &mut sink, None).unwrap().event,
            Event::Need
        ));
        assert!(matches!(
            dec.step(Input::new(0, &bytes), &mut sink, None)
                .unwrap()
                .event,
            Event::Need
        ));
        assert_eq!(
            dec.requested(),
            &[InputRequest::at_least(0, bytes.len() + 1)]
        );
        let eof = Input::new(0, &bytes).with_total_len(bytes.len() as u64);
        assert!(matches!(
            dec.step(eof, &mut sink, None).unwrap().event,
            Event::Rows { .. }
        ));
        assert_eq!(sink.begins, 1);
        assert_eq!(sink.finishes, 1);
        let actual: Vec<u8> = bytemuck::cast_slice::<_, u8>(&sink.storage)
            .chunks_exact(sink.stride)
            .flat_map(|row| {
                row[..sink.width as usize * desc.bytes_per_pixel()]
                    .iter()
                    .copied()
            })
            .collect();
        assert_eq!(actual, expected_bytes);
        for row in bytemuck::cast_slice::<_, u8>(&sink.storage).chunks_exact(sink.stride) {
            assert!(
                row[sink.width as usize * desc.bytes_per_pixel()..]
                    .iter()
                    .all(|&b| b == 0xA5),
                "decoder must leave caller padding untouched"
            );
        }
        assert!(matches!(
            dec.step(eof, &mut sink, None).unwrap().event,
            Event::Done(_)
        ));
        assert!(matches!(
            dec.step(Input::empty(), &mut sink, None).unwrap().event,
            Event::Done(_)
        ));
    }

    #[test]
    fn header_admission_and_input_plus_output_memory_precede_header_and_sink() {
        let bytes = fixture::bytes();
        let mut dec = decoder(ResourceLimits::none().with_max_pixels(0));
        assert_denial(&mut *dec, Input::new(0, &bytes), RawLimitKind::Pixels);
        let expected = RawDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(&bytes), &[])
            .unwrap()
            .decode()
            .unwrap();
        let output = u64::from(expected.width())
            * u64::from(expected.height())
            * expected.descriptor().bytes_per_pixel() as u64;
        let mut dec =
            decoder(ResourceLimits::none().with_max_memory(bytes.len() as u64 + output - 1));
        assert_denial(
            &mut *dec,
            Input::new(0, &bytes).with_total_len(bytes.len() as u64),
            RawLimitKind::Memory,
        );
    }

    #[test]
    fn per_step_cancellation_precedes_probe_and_is_terminal() {
        struct Stopped;
        impl Stop for Stopped {
            fn check(&self) -> Result<(), StopReason> {
                Err(StopReason::Cancelled)
            }
        }
        let bytes = fixture::bytes();
        let mut dec = decoder(ResourceLimits::none());
        let mut sink = Sink::default();
        assert!(matches!(
            dec.step(
                Input::new(0, &bytes),
                &mut sink,
                Some(&how_far::StopOnly::new(Stopped))
            ),
            Err(StepError::Cancelled(StopReason::Cancelled))
        ));
        assert!(dec.info().is_none());
        assert_eq!(sink.begins, 0);
        assert!(matches!(
            dec.step(Input::empty(), &mut sink, None),
            Err(StepError::Finished)
        ));
    }

    #[test]
    #[allow(deprecated)] // Retain coverage of the explicit-stop compatibility path.
    fn transient_job_stop_after_preflight_cannot_be_swallowed_by_a_second_probe() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct CountStops {
            calls: Arc<AtomicUsize>,
            stop_at: usize,
        }
        impl Stop for CountStops {
            fn check(&self) -> Result<(), StopReason> {
                if self.calls.fetch_add(1, Ordering::Relaxed) == self.stop_at {
                    Err(StopReason::Cancelled)
                } else {
                    Ok(())
                }
            }
        }
        let bytes = fixture::bytes();
        let prefix = &bytes[..bytes.len() - 1];
        let calls = Arc::new(AtomicUsize::new(0));
        RawDecoderConfig::new()
            .job()
            .with_stop(zencodec::StopToken::new(CountStops {
                calls: calls.clone(),
                stop_at: usize::MAX,
            }))
            .probe(prefix)
            .unwrap();
        let native_probe_calls = calls.swap(0, Ordering::Relaxed);
        assert!(native_probe_calls > 0);
        let mut dec = RawDecoderConfig::new()
            .job()
            .with_stop(zencodec::StopToken::new(CountStops {
                calls: calls.clone(),
                stop_at: native_probe_calls,
            }))
            .dyn_incremental_decoder(&[])
            .unwrap();
        let mut sink = Sink::default();
        let error = dec
            .step(Input::new(0, prefix), &mut sink, None)
            .unwrap_err();
        let StepError::Codec(error) = error else {
            panic!("unexpected error: {error:?}")
        };
        let native = error
            .get_ref()
            .downcast_ref::<whereat::At<RawError>>()
            .unwrap();
        assert!(matches!(
            native.error(),
            RawError::Stopped(StopReason::Cancelled)
        ));
        assert_eq!(calls.load(Ordering::Relaxed), native_probe_calls + 1);
        assert!(
            dec.info().is_none(),
            "unreported helper header must stay private on denial"
        );
        assert_eq!(sink.begins, 0);
        assert!(matches!(
            dec.step(Input::empty(), &mut sink, None),
            Err(StepError::Finished)
        ));
    }

    #[test]
    fn truncated_eof_is_not_reported_as_need_or_header() {
        let bytes = fixture::bytes();
        for end in [0, 7] {
            let mut dec = decoder(ResourceLimits::none());
            let mut sink = Sink::default();
            assert!(
                dec.step(
                    Input::new(0, &bytes[..end]).with_total_len(end as u64),
                    &mut sink,
                    None
                )
                .is_err()
            );
            assert!(dec.info().is_none());
            assert_eq!(sink.begins, 0);
            assert!(matches!(
                dec.step(Input::empty(), &mut sink, None),
                Err(StepError::Finished)
            ));
        }
    }
}
