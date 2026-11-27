//! Native progress/cancellation contracts, exercised with either RAW backend.
#![cfg(any(feature = "rawloader", feature = "rawler"))]

#[path = "support/dng_builder.rs"]
mod dng_builder;
use dng_builder::*;
use how_far::{Complete, NoPulse, Outcome, Stop, StopOnly, StopReason, Total};
use how_far_along::{FnPulse, Phase, PulseTree, Status};
use zenraw::{DemosaicMethod, OutputMode, RawDecodeConfig, RawError};

fn values(tag: u16, kind: u16, count: u32, value: Vec<u8>) -> Entry {
    Entry {
        tag,
        kind,
        count,
        value,
    }
}

fn fixture(w: u16, h: u16, rgb: bool, float: bool, xtrans: bool, orientation: u16) -> Vec<u8> {
    let cpp = if rgb { 3 } else { 1 };
    let mut entries = vec![
        short(0x100, w),
        short(0x101, h),
        short(0x102, if float { 32 } else { 16 }),
        short(0x103, 1),
        short(0x106, if rgb { 34892 } else { 32803 }),
        ascii(0x10f, "ZenRaw"),
        ascii(0x110, "PulseFixture"),
        long(0x111, 0),
        short(0x112, orientation),
        short(0x115, cpp),
        long(0x116, h as u32),
        long(
            0x117,
            w as u32 * h as u32 * cpp as u32 * if float { 4 } else { 2 },
        ),
        short(0x11c, 1),
        short(0x153, if float { 3 } else { 1 }),
        bytes(0xc612, &[1, 4, 0, 0]),
        long(0xc61d, if float { 1 } else { 65535 }),
        // A real color matrix and neutral white balance exercise rawler's
        // DngPipeline branch as well as rawloader's basic color pipeline.
        values(
            0xc621,
            10,
            9,
            [1i32, 0, 0, 0, 1, 0, 0, 0, 1]
                .iter()
                .flat_map(|n| [n.to_le_bytes(), 1i32.to_le_bytes()].concat())
                .collect(),
        ),
        values(
            0xc628,
            5,
            3,
            [1u32, 1, 1]
                .iter()
                .flat_map(|n| [n.to_le_bytes(), 1u32.to_le_bytes()].concat())
                .collect(),
        ),
        values(
            0xc61f,
            LONG,
            2,
            [1u32, 1].iter().flat_map(|n| n.to_le_bytes()).collect(),
        ),
        values(
            0xc620,
            LONG,
            2,
            [w as u32 - 2, h as u32 - 2]
                .iter()
                .flat_map(|n| n.to_le_bytes())
                .collect(),
        ),
    ];
    if !rgb {
        let size = if xtrans { 6u16 } else { 2 };
        entries.push(values(
            0x828d,
            SHORT,
            2,
            [size, size].iter().flat_map(|n| n.to_le_bytes()).collect(),
        ));
        entries.push(bytes(
            0x828e,
            if xtrans {
                &[
                    1, 0, 1, 1, 0, 1, 2, 1, 2, 0, 1, 0, 1, 0, 1, 1, 0, 1, 1, 2, 1, 1, 2, 1, 0, 1,
                    0, 2, 1, 2, 1, 2, 1, 1, 2, 1,
                ]
            } else {
                &[0, 1, 1, 2]
            },
        ));
    }
    let mut strip = Vec::new();
    for i in 0..w as usize * h as usize * cpp as usize {
        let sample = (i * 61 % 65535) as u16;
        if float {
            strip.extend_from_slice(&(sample as f32 / 65535.0).to_le_bytes());
        } else {
            strip.extend_from_slice(&sample.to_le_bytes());
        }
    }
    strip.extend_from_slice(&[0; 64]);
    build_tiff(entries, &strip, 0x111)
}

fn stopped(error: &whereat::At<RawError>) -> bool {
    matches!(error.error(), RawError::Stopped(_))
}

#[test]
fn tracked_decode_counts_exact_work_and_preserves_pixels() {
    for (rgb, float, xtrans) in [
        (false, false, false),
        (false, false, true),
        (true, false, false),
        // rawloader 0.37.2 rejects uncompressed 32-bit float DNG samples.
        #[cfg(feature = "rawler")]
        (true, true, false),
        #[cfg(feature = "rawler")]
        (false, true, false),
    ] {
        for mode in [
            OutputMode::CameraRaw,
            OutputMode::Linear,
            OutputMode::LinearExtended,
            OutputMode::Develop,
        ] {
            for orientation in 1..=8 {
                for method in [DemosaicMethod::Bilinear, DemosaicMethod::MalvarHeCutler] {
                    let data = fixture(131, 99, rgb, float, xtrans, orientation);
                    let config = RawDecodeConfig::new()
                        .with_output(mode)
                        .with_demosaic(method)
                        .with_exposure_ev(if orientation % 2 == 0 { 0.0 } else { 0.5 })
                        .with_crop(orientation % 3 != 0);
                    let base = zenraw::decode(&data, &config, None).expect("unobserved decode");
                    let no_pulse = zenraw::decode(&data, &config, Some(&NoPulse)).unwrap();
                    let tree =
                        PulseTree::new(Phase::new("decode", Total::Unknown), how_far::Unstoppable);
                    let observer = tree.observer();
                    let tracked = zenraw::decode(&data, &config, Some(&tree)).unwrap();
                    let snapshot = observer.snapshot();
                    assert_eq!(
                        snapshot.status,
                        Status::Running,
                        "borrowed root must stay open"
                    );
                    assert_eq!(snapshot.children.len(), 7);
                    for child in &snapshot.children {
                        match child.status {
                            Status::Finished(Outcome::Succeeded) => assert_eq!(
                                child.total,
                                Total::Exact(child.completed),
                                "{rgb}/{float}/{xtrans}/{mode:?}/{orientation}/{method:?}: {child:?}"
                            ),
                            Status::Finished(Outcome::Skipped) => assert_eq!(child.completed, 0),
                            _ => panic!("unfinished child: {child:?}"),
                        }
                        assert!(!child.overrun(), "{child:?}");
                    }
                    assert_eq!(
                        base.pixels.as_view().to_contiguous_bytes().unwrap(),
                        tracked.pixels.as_view().to_contiguous_bytes().unwrap()
                    );
                    assert_eq!(
                        base.pixels.as_view().to_contiguous_bytes().unwrap(),
                        no_pulse.pixels.as_view().to_contiguous_bytes().unwrap()
                    );
                    assert_eq!(
                        (base.info.width, base.info.height),
                        (tracked.info.width, tracked.info.height)
                    );
                    tree.complete_classified(Ok::<_, whereat::At<RawError>>(()), stopped)
                        .unwrap();
                    assert_eq!(
                        observer.snapshot().status,
                        Status::Finished(Outcome::Succeeded)
                    );
                }
            }
        }
    }
}

#[test]
fn progress_callbacks_can_cancel_inside_batches_and_after_final_report() {
    for target in [
        "Normalize",
        "Reconstruct RGB",
        "Crop",
        "Orient",
        "Render output",
    ] {
        for reason in [StopReason::Cancelled, StopReason::TimedOut] {
            let data = fixture(1024, 769, false, false, false, 6);
            let tree = FnPulse::new("decode", move |cx| {
                if cx.phase_name() == target {
                    let snapshot = cx.snapshot();
                    let child = snapshot.children.iter().find(|c| c.name == target).unwrap();
                    if child.completed > 0 {
                        return Err(reason);
                    }
                }
                Ok(())
            });
            let error =
                zenraw::decode(&data, &RawDecodeConfig::default(), Some(&tree)).unwrap_err();
            assert!(matches!(error.error(), RawError::Stopped(r) if *r == reason));
            let snapshot = tree.observer().snapshot();
            let index = snapshot
                .children
                .iter()
                .position(|c| c.name == target)
                .unwrap();
            assert_eq!(
                snapshot.children[index].status,
                Status::Finished(Outcome::Cancelled)
            );
            for child in &snapshot.children[index + 1..] {
                assert_eq!(child.status, Status::Finished(Outcome::NotRun));
            }
            tree.complete_classified(Err::<(), _>(error), stopped)
                .unwrap_err();
        }
    }
}

#[test]
fn cancelled_parse_does_not_run_later_stages_and_probe_stays_borrowed() {
    let data = fixture(17, 19, false, false, false, 1);
    let tree = FnPulse::new("decode", |cx| {
        if cx.phase_name() == "Parse RAW" {
            Err(StopReason::Cancelled)
        } else {
            Ok(())
        }
    });
    assert!(zenraw::decode(&data, &RawDecodeConfig::default(), Some(&tree)).is_err());
    let snapshot = tree.observer().snapshot();
    assert_eq!(snapshot.children[0].completed, 0);
    assert_eq!(
        snapshot.children[0].status,
        Status::Finished(Outcome::Cancelled)
    );
    assert!(
        snapshot.children[1..]
            .iter()
            .all(|c| c.status == Status::Finished(Outcome::NotRun))
    );
    tree.complete_as(Outcome::Cancelled);

    let tree = PulseTree::new(Phase::new("probe", Total::Unknown), how_far::Unstoppable);
    let observer = tree.observer();
    zenraw::probe(&data, Some(&tree)).unwrap();
    let snapshot = observer.snapshot();
    assert_eq!(snapshot.status, Status::Running);
    assert_eq!(snapshot.children[0].total, Total::Exact(1));
    assert_eq!(snapshot.children[0].completed, 1);
    tree.complete_as(Outcome::Succeeded);
}

#[test]
fn a_single_non_sticky_stop_is_never_lost() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Once {
        calls: AtomicUsize,
        at: usize,
    }
    impl Stop for Once {
        fn check(&self) -> Result<(), StopReason> {
            if self.calls.fetch_add(1, Ordering::Relaxed) == self.at {
                Err(StopReason::TimedOut)
            } else {
                Ok(())
            }
        }
    }
    for rgb in [false, true] {
        let data = fixture(131, 99, rgb, false, false, 3);
        let config = RawDecodeConfig::default();
        let counter = Once {
            calls: AtomicUsize::new(0),
            at: usize::MAX,
        };
        zenraw::decode(&data, &config, Some(&StopOnly::borrowed(&counter))).unwrap();
        for at in 0..counter.calls.load(Ordering::Relaxed) {
            let once = Once {
                calls: AtomicUsize::new(0),
                at,
            };
            let error =
                zenraw::decode(&data, &config, Some(&StopOnly::borrowed(&once))).unwrap_err();
            assert!(
                matches!(error.error(), RawError::Stopped(StopReason::TimedOut)),
                "{rgb}/{at}: {error}"
            );
        }
    }
}

#[test]
fn probe_checks_after_its_final_report_and_failures_do_not_earn_success() {
    let data = fixture(17, 19, false, false, false, 1);
    let pulse = FnPulse::new("probe", |cx| {
        if cx.snapshot().children.iter().any(|c| c.completed == 1) {
            Err(StopReason::TimedOut)
        } else {
            Ok(())
        }
    });
    let error = zenraw::probe(&data, Some(&pulse)).unwrap_err();
    assert!(matches!(
        error.error(),
        RawError::Stopped(StopReason::TimedOut)
    ));
    assert_eq!(
        pulse.observer().snapshot().children[0].status,
        Status::Finished(Outcome::Cancelled)
    );
    pulse
        .complete_classified(Err::<(), _>(error), stopped)
        .unwrap_err();

    let pulse = PulseTree::new(
        Phase::new("bad input", Total::Unknown),
        how_far::Unstoppable,
    );
    let error =
        zenraw::decode(&[0xff; 128], &RawDecodeConfig::default(), Some(&pulse)).unwrap_err();
    let snapshot = pulse.observer().snapshot();
    assert_eq!(
        snapshot.children[0].status,
        Status::Finished(Outcome::Failed)
    );
    assert!(
        snapshot.children[1..]
            .iter()
            .all(|c| c.status == Status::Finished(Outcome::NotRun))
    );
    pulse
        .complete_classified(Err::<(), _>(error), stopped)
        .unwrap_err();
}

#[cfg(feature = "zencodec")]
mod adapter {
    use super::*;
    use how_far::Pulse;
    use std::borrow::Cow;
    use zencodec::OrientationHint;
    use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
    use zenraw::RawDecoderConfig;

    #[test]
    fn shared_pulse_survives_repeated_probes_decode_and_output_orientation() {
        let data = fixture(131, 99, false, false, false, 6);
        for hint in [OrientationHint::Preserve, OrientationHint::Correct] {
            let config = RawDecoderConfig::new();
            let base = config
                .clone()
                .job()
                .with_orientation(hint)
                .decoder(Cow::Borrowed(&data), &[])
                .unwrap()
                .decode()
                .unwrap();
            let pulse = PulseTree::new(Phase::new("adapter", Total::Unknown), how_far::Unstoppable);
            let observer = pulse.observer();
            let job = config
                .job()
                .with_pulse(pulse.share().unwrap())
                .with_orientation(hint);
            job.probe(&data).unwrap();
            job.output_info(&data).unwrap();
            job.probe(&data).unwrap();
            assert!(
                observer.snapshot().children.is_empty(),
                "probes must leave the decode plan available"
            );
            let out = job
                .decoder(Cow::Borrowed(&data), &[])
                .unwrap()
                .decode()
                .unwrap();
            assert_eq!(
                base.pixels().to_contiguous_bytes().unwrap(),
                out.pixels().to_contiguous_bytes().unwrap()
            );
            assert_eq!(
                (base.info().width, base.info().height),
                (out.info().width, out.info().height)
            );
            let snap = observer.snapshot();
            assert_eq!(snap.status, Status::Running);
            assert_eq!(
                snap.children[0].status,
                Status::Finished(Outcome::Succeeded)
            );
            assert_eq!(snap.children[0].children.len(), 7);
            assert_eq!(
                snap.children[1].status,
                Status::Finished(if hint == OrientationHint::Preserve {
                    Outcome::Skipped
                } else {
                    Outcome::Succeeded
                })
            );
            if hint != OrientationHint::Preserve {
                assert_eq!(
                    snap.children[1].total,
                    Total::Exact(snap.children[1].completed)
                );
                assert!(snap.children[1].completed > 0);
            }
            pulse.complete_as(Outcome::Succeeded);
        }
    }

    #[derive(Default)]
    struct PaddedSink {
        storage: Vec<u32>,
        stride: usize,
        finished: bool,
        metadata: Option<zencodec::decode::RowMetadata>,
    }

    impl zencodec::decode::DecodeRowSink for PaddedSink {
        fn begin(
            &mut self,
            w: u32,
            h: u32,
            format: zenpixels::PixelFormat,
        ) -> Result<(), zencodec::decode::SinkError> {
            let row_bytes = w as usize * format.layout.bytes_per_pixel();
            self.stride = (row_bytes + 1).next_multiple_of(4);
            self.storage
                .resize(self.stride * h as usize / 4, 0xa5a5_a5a5);
            Ok(())
        }

        fn provide_next_buffer(
            &mut self,
            y: u32,
            h: u32,
            w: u32,
            format: zenpixels::PixelFormat,
        ) -> Result<zenpixels::PixelViewMut<'_>, zencodec::decode::SinkError> {
            let bytes = bytemuck::cast_slice_mut::<u32, u8>(&mut self.storage);
            zenpixels::PixelViewMut::new_with_format_cow(
                &mut bytes[y as usize * self.stride..],
                w,
                h,
                self.stride,
                zenpixels::FormatCow::from_owned(format),
            )
            .map_err(|e| e.to_string().into())
        }

        fn max_buffer_rows(&self) -> std::num::NonZeroU32 {
            std::num::NonZeroU32::new(3).unwrap()
        }

        fn provide_buffer_with_metadata(
            &mut self,
            meta: zencodec::decode::BatchMeta,
            height: u32,
            width: u32,
            format: zenpixels::PixelFormat,
            metadata: &zencodec::decode::RowMetadata,
        ) -> Result<zenpixels::PixelViewMut<'_>, zencodec::decode::SinkError> {
            assert!(meta.is_final && height <= self.max_buffer_rows().get());
            if let Some(held) = &mut self.metadata {
                held.merge(metadata)?;
            } else {
                self.metadata = Some(metadata.clone());
            }
            self.provide_next_buffer(meta.y, height, width, format)
        }

        fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
            self.finished = true;
            Ok(())
        }
    }

    impl PaddedSink {
        fn assert_pixels(&self, expected: &zenpixels::PixelView<'_>) {
            assert!(self.finished);
            assert_eq!(
                self.metadata.as_ref(),
                Some(&zencodec::decode::RowMetadata::from_view(expected))
            );
            let bytes = bytemuck::cast_slice::<u32, u8>(&self.storage);
            assert_eq!(bytes.len(), self.stride * expected.height() as usize);
            for (y, row) in bytes.chunks_exact(self.stride).enumerate() {
                let (active, padding) = row.split_at(expected.row_bytes());
                assert_eq!(active, expected.row(y as u32));
                assert!(!padding.is_empty());
                assert!(
                    padding.iter().all(|&byte| byte == 0xa5),
                    "row {y} padding changed"
                );
            }
        }
    }

    #[test]
    fn native_and_zencodec_match_for_each_exif_orientation() {
        use zencodec::Orientation;
        use zencodec::decode::{Event, Input};

        for exif in 1..=8 {
            let orientation = Orientation::from_exif(exif).unwrap();
            let data = fixture(17, 19, false, false, false, u16::from(exif));
            for (mode, target) in [
                (OutputMode::Develop, zenraw::OutputPrimaries::DisplayP3),
                (OutputMode::Linear, zenraw::OutputPrimaries::Bt2020),
                (
                    OutputMode::LinearExtended,
                    zenraw::OutputPrimaries::DisplayP3,
                ),
                (OutputMode::CameraRaw, zenraw::OutputPrimaries::Srgb),
            ] {
                for (bake, hint) in [
                    (false, OrientationHint::Preserve),
                    (true, OrientationHint::Correct),
                ] {
                    let config = RawDecodeConfig::new()
                        .with_output(mode)
                        .with_target(target)
                        .with_orientation(bake);
                    let native = zenraw::decode(&data, &config, None).unwrap();
                    let adapter = RawDecoderConfig::from_config(config);
                    let job = adapter.clone().job().with_orientation(hint);
                    let prediction = job.output_info(&data).unwrap();
                    let decoded = job
                        .decoder(Cow::Borrowed(&data), &[])
                        .unwrap()
                        .decode()
                        .unwrap();
                    let expected = native.pixels.as_view();
                    let actual = decoded.pixels();
                    assert_eq!(
                        (actual.width(), actual.height()),
                        (expected.width(), expected.height())
                    );
                    assert_eq!(actual.format(), expected.format());
                    assert_eq!(actual.facts(), expected.facts());
                    assert_eq!(
                        actual.to_contiguous_bytes().unwrap(),
                        expected.to_contiguous_bytes().unwrap()
                    );
                    let (pending, applied) = if bake {
                        (Orientation::Identity, orientation)
                    } else {
                        (orientation, Orientation::Identity)
                    };
                    assert_eq!(native.info.orientation, u16::from(pending.to_exif()));
                    assert_eq!(decoded.info().orientation, pending);
                    assert_eq!(prediction.orientation_applied, applied);

                    let mut sink = PaddedSink::default();
                    let mut incremental = adapter
                        .job()
                        .with_orientation(hint)
                        .dyn_incremental_decoder(&[])
                        .unwrap();
                    let input = || Input::new(0, &data).with_total_len(data.len() as u64);
                    assert!(matches!(
                        incremental.step(input(), &mut sink, None).unwrap().event,
                        Event::Metadata(_)
                    ));
                    assert!(matches!(
                        incremental.step(input(), &mut sink, None).unwrap().event,
                        Event::Rows { .. }
                    ));
                    let Event::Done(info) = incremental
                        .step(Input::empty(), &mut sink, None)
                        .unwrap()
                        .event
                    else {
                        panic!("incremental decode must finish after delivering the rows");
                    };
                    assert_eq!(info, prediction);
                    assert_eq!(info.native_format, *expected.format());
                    assert_eq!(
                        (info.width, info.height),
                        (expected.width(), expected.height())
                    );
                    sink.assert_pixels(&expected);
                }
            }
        }
    }

    #[test]
    fn incremental_decode_retains_shared_progress_and_exact_sink_pixels() {
        use zencodec::decode::{Event, Input};
        let data = fixture(131, 99, false, false, false, 6);
        let config = RawDecoderConfig::new();
        let expected = config
            .clone()
            .job()
            .decoder(Cow::Borrowed(&data), &[])
            .unwrap()
            .decode()
            .unwrap();
        let pulse = PulseTree::new(
            Phase::new("incremental", Total::Unknown),
            how_far::Unstoppable,
        );
        let observer = pulse.observer();
        let mut decoder = config
            .job()
            .with_pulse(pulse.share().unwrap())
            .dyn_incremental_decoder(&[])
            .unwrap();
        let mut sink = PaddedSink::default();
        let input = || Input::new(0, &data).with_total_len(data.len() as u64);
        assert!(matches!(
            decoder.step(input(), &mut sink, None).unwrap().event,
            Event::Metadata(_)
        ));
        assert!(observer.snapshot().children.is_empty());
        assert!(matches!(
            decoder.step(input(), &mut sink, None).unwrap().event,
            Event::Rows { .. }
        ));
        assert!(matches!(
            decoder.step(Input::empty(), &mut sink, None).unwrap().event,
            Event::Done(_)
        ));
        sink.assert_pixels(&expected.pixels());
        let snap = observer.snapshot();
        assert_eq!(snap.status, Status::Running);
        assert_eq!(snap.children[0].children.len(), 7);
        assert_eq!(
            snap.children[0].status,
            Status::Finished(Outcome::Succeeded)
        );
        pulse.complete_as(Outcome::Succeeded);
    }

    #[test]
    #[allow(deprecated)] // Exercise the retained explicit-stop compatibility path.
    fn either_pulse_or_explicit_stop_cancels_the_adapter() {
        let data = fixture(131, 99, false, false, false, 6);
        for (explicit_stop, target) in [
            (false, "Normalize"),
            (false, "Output orientation"),
            (true, "Normalize"),
        ] {
            let pulse = FnPulse::new("adapter", move |cx| {
                if !explicit_stop
                    && cx.phase_name() == target
                    && (target == "Normalize"
                        || cx
                            .snapshot()
                            .children
                            .iter()
                            .any(|c| c.name == target && c.completed > 0))
                {
                    Err(StopReason::TimedOut)
                } else {
                    Ok(())
                }
            });
            let shared = pulse.share().unwrap();
            let mut job = RawDecoderConfig::new()
                .job()
                .with_pulse(shared)
                .with_orientation(OrientationHint::Correct);
            struct Cancelled;
            impl Stop for Cancelled {
                fn check(&self) -> Result<(), StopReason> {
                    Err(StopReason::Cancelled)
                }
            }
            if explicit_stop {
                job = job.with_stop(zencodec::StopToken::new(Cancelled));
            }
            let result = job
                .decoder(Cow::Borrowed(&data), &[])
                .and_then(|d| d.decode());
            let error =
                result.expect_err(&format!("explicit_stop={explicit_stop}, target={target}"));
            assert!(
                matches!(error.error(), RawError::Stopped(r) if *r == if explicit_stop {StopReason::Cancelled} else {StopReason::TimedOut})
            );
            pulse
                .complete_classified(Err::<(), _>(error), stopped)
                .unwrap_err();
        }
    }
    #[test]
    fn incremental_header_then_job_pulse_timeout_refuses_before_sink_effects() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use zencodec::decode::{DecodeRowSink, Event, Input, SinkError, StepError};
        use zenpixels::{PixelFormat, PixelViewMut};
        struct NoEffects;
        impl DecodeRowSink for NoEffects {
            fn begin(&mut self, _: u32, _: u32, _: PixelFormat) -> Result<(), SinkError> {
                panic!("cancelled session published a header")
            }
            fn provide_next_buffer(
                &mut self,
                _: u32,
                _: u32,
                _: u32,
                _: PixelFormat,
            ) -> Result<PixelViewMut<'_>, SinkError> {
                panic!("cancelled session requested output storage")
            }
            fn finish(&mut self) -> Result<(), SinkError> {
                panic!("cancelled session finished output")
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let pulse = FnPulse::new("incremental timeout", move |_| {
            if flag.load(Ordering::SeqCst) {
                Err(StopReason::TimedOut)
            } else {
                Ok(())
            }
        });
        let observer = pulse.observer();
        let data = fixture(131, 99, false, false, false, 6);
        let mut decoder = RawDecoderConfig::new()
            .job()
            .with_pulse(pulse.share().unwrap())
            .dyn_incremental_decoder(&[])
            .unwrap();
        let input = || Input::new(0, &data).with_total_len(data.len() as u64);
        let mut sink = NoEffects;
        assert!(matches!(
            decoder.step(input(), &mut sink, None).unwrap().event,
            Event::Metadata(_)
        ));
        cancelled.store(true, Ordering::SeqCst);
        let error = decoder.step(Input::empty(), &mut sink, None).unwrap_err();
        assert!(
            matches!(error, StepError::Codec(ref e) if matches!(zencodec::find_cause::<RawError>(e), Some(RawError::Stopped(StopReason::TimedOut))))
        );
        assert!(
            observer.snapshot().children.is_empty(),
            "cancelled preflight must not consume decode plan"
        );
        drop(decoder);
        pulse.complete_as(Outcome::Cancelled);
    }
}
